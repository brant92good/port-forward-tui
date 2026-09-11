use port_forward_tui::{
    background, channel,
    machines::Catalog,
    process,
    store::{Forward, Settings, Store},
};
use serde_json::{Value, json};
use std::{
    fs,
    net::{Ipv4Addr, TcpListener},
    time::Duration,
};

#[test]
fn local_shape_and_socks_schema_are_unambiguous_and_sticky() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::load(temp.path()).unwrap();
    let local = Forward::new(18000, 8000, "API").unwrap();
    let value = serde_json::to_value(&local).unwrap();
    assert_eq!(
        value,
        json!({"id":local.id,"name":"API","local_port":18000,"remote_port":8000})
    );
    store.save(vec![local.clone()]).unwrap();
    assert_eq!(store.settings.version, 1);
    let proxy = Forward::socks(1080, "Browser").unwrap();
    assert_eq!(
        serde_json::to_value(&proxy).unwrap(),
        json!({"id":proxy.id,"name":"Browser","local_port":1080,"kind":"socks"})
    );
    store.save(vec![local.clone(), proxy.clone()]).unwrap();
    assert_eq!(Store::load(temp.path()).unwrap().settings.version, 2);
    store.save(vec![local]).unwrap();
    assert_eq!(Store::load(temp.path()).unwrap().settings.version, 2);
    assert!(!proxy.same_mapping(&Forward::new(1080, 1080, "Local").unwrap()));
    for invalid in [
        json!({"kind":"unknown"}),
        json!({"kind":"socks","remote_port":8000}),
        json!({"kind":"local"}),
    ] {
        let mut raw = json!({"id":proxy.id,"name":"test","local_port":1080});
        raw.as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert!(
            serde_json::from_value::<Forward>(raw)
                .and_then(|rule| rule
                    .validate()
                    .map(|_| rule)
                    .map_err(serde::de::Error::custom))
                .is_err()
        );
    }
    let old_schema = Settings {
        version: 1,
        forwards: vec![proxy.clone()],
        ..Settings::default()
    };
    assert!(old_schema.validate().is_err());
    let mut raw = serde_json::to_value(&proxy).unwrap();
    raw["remote_port"] = Value::Null;
    let normalized: Forward = serde_json::from_value(raw).unwrap();
    normalized.validate().unwrap();
    assert!(
        serde_json::to_value(normalized)
            .unwrap()
            .get("remote_port")
            .is_none()
    );
}

#[test]
fn socks_arguments_preserve_strict_transport_and_do_not_invent_destination() {
    let settings = Settings {
        host: "test-host".into(),
        ssh_port: Some(2222),
        ssh_config: Some("space dir/config".into()),
        ..Settings::default()
    };
    let proxy = Forward::socks(1080, "Proxy").unwrap();
    let args = process::ssh_arguments(&settings, &proxy).unwrap();
    assert_eq!(&args[..4], ["-p", "2222", "-F", "space dir/config"]);
    assert_eq!(
        &args[args.len() - 3..],
        ["-D", "127.0.0.1:1080", "test-host"]
    );
    for flag in [
        "-N",
        "-T",
        "StrictHostKeyChecking=yes",
        "BatchMode=yes",
        "ControlMaster=no",
        "ForkAfterAuthentication=no",
    ] {
        assert!(args.iter().any(|arg| arg == flag));
    }
    assert!(!args.iter().any(|arg| arg == "-L" || arg == "-f"));
    let mut invalid = proxy;
    invalid.remote_port = Some(8000);
    assert!(process::ssh_arguments(&settings, &invalid).is_err());
}

#[test]
fn metadata_import_is_explicit_transactional_and_leaves_active_state_behind() {
    let temp = tempfile::tempdir().unwrap();
    let stable = temp.path().join("stable");
    fs::create_dir(&stable).unwrap();
    let rule = Forward::new(18000, 8000, "Unicode 開發").unwrap();
    let settings = Settings {
        host: "fixture".into(),
        forwards: vec![rule.clone()],
        ssh_config: Some("custom path/config".into()),
        ..Settings::default()
    };
    let original = serde_json::to_vec(&settings).unwrap();
    fs::write(stable.join("forwards.json"), &original).unwrap();
    fs::write(
        stable.join("endpoint.json"),
        br#"{"protocol":1,"pid":7,"port":99,"token":"private"}"#,
    )
    .unwrap();
    fs::write(
        stable.join("forward-options.json"),
        serde_json::to_vec(&json!({"version":1,"open_automatically":[rule.id]})).unwrap(),
    )
    .unwrap();
    fs::write(stable.join("daemon.lock"), b"live").unwrap();
    fs::create_dir(stable.join("views")).unwrap();
    assert!(channel::validate_directory(&stable).is_err());
    assert!(Store::load(&stable).unwrap().save(vec![]).is_err());
    let beta = temp.path().join("beta");
    let machines = channel::import_stable(&stable, &beta).unwrap();
    assert_eq!(machines.len(), 1);
    assert_eq!(machines[0].directory, beta);
    assert_eq!(fs::read(beta.join("forwards.json")).unwrap(), original);
    assert_eq!(fs::read(stable.join("forwards.json")).unwrap(), original);
    assert_eq!(fs::read_dir(&beta).unwrap().count(), 2);
    assert!(
        !port_forward_tui::auto_open::Preferences::load(&beta)
            .unwrap()
            .enabled(&rule.id)
    );
    let mut edited = Store::load(&beta).unwrap();
    edited
        .save(vec![Forward::socks(1080, "Beta edit").unwrap()])
        .unwrap();
    let changed = fs::read(beta.join("forwards.json")).unwrap();
    assert!(channel::import_stable(&stable, &beta).is_err());
    assert_eq!(fs::read(beta.join("forwards.json")).unwrap(), changed);
    assert_eq!(fs::read(stable.join("forwards.json")).unwrap(), original);
    let empty = temp.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(
        channel::import_stable(&empty, &temp.path().join("empty-import"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn wrong_protocol_fails_before_network_writes_or_launch_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let machine = Catalog::new(temp.path())
        .unwrap()
        .add("fixture", "", None, None)
        .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = json!({"protocol":1,"pid":std::process::id(),"port":listener.local_addr().unwrap().port(),"token":"a".repeat(64)});
    fs::write(
        machine.directory.join("endpoint.json"),
        serde_json::to_vec(&endpoint).unwrap(),
    )
    .unwrap();
    let before = fs::read(machine.directory.join("forwards.json")).unwrap();
    for action in ["status", "upsert", "delete", "stop", "stop_all", "shutdown"] {
        assert!(background::call(&machine.directory, action, json!({})).is_err());
        assert!(
            background::exchange(
                &machine.directory,
                action,
                json!({}),
                Duration::from_millis(100)
            )
            .is_err()
        );
    }
    assert!(background::launch(&machine.directory).is_err());
    assert!(listener.accept().is_err());
    assert_eq!(
        before,
        fs::read(machine.directory.join("forwards.json")).unwrap()
    );
    assert!(
        !machine
            .directory
            .join("port_forward_tui.background.log")
            .exists()
    );
    assert!(!machine.directory.join("daemon.lock").exists());
}

#[cfg(windows)]
#[test]
fn failed_schema_promotion_keeps_old_disk_and_memory() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::load(temp.path()).unwrap();
    let local = Forward::new(18000, 8000, "API").unwrap();
    store.save(vec![local.clone()]).unwrap();
    let before = fs::read(store.path()).unwrap();
    let reader = fs::File::open(store.path()).unwrap();
    let result = store.save(vec![local.clone(), Forward::socks(1080, "Proxy").unwrap()]);
    assert!(result.is_err());
    assert_eq!(store.settings.version, 1);
    assert_eq!(store.settings.forwards, vec![local]);
    assert_eq!(fs::read(store.path()).unwrap(), before);
    drop(reader);
}

#[cfg(windows)]
#[test]
fn windows_junctions_cannot_import_or_write_through_another_data_tree() {
    use std::{os::windows::process::CommandExt, process::Command};
    fn junction(link: &std::path::Path, target: &std::path::Path) {
        let result = Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    junction(&source.join("machines"), &outside);
    assert!(channel::import_stable(&source, &temp.path().join("beta")).is_err());
    assert!(!temp.path().join("beta").exists());
    fs::remove_dir(source.join("machines")).unwrap();
    let original = serde_json::to_vec(&Settings {
        host: "fixture".into(),
        ..Settings::default()
    })
    .unwrap();
    fs::write(outside.join("forwards.json"), &original).unwrap();
    let alias = temp.path().join("alias");
    junction(&alias, &outside);
    assert!(channel::prepare(&alias).is_err());
    assert_eq!(fs::read(outside.join("forwards.json")).unwrap(), original);
    assert!(!outside.join(channel::MARKER).exists());
    fs::remove_dir(alias).unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_import_and_write_boundaries_preserve_outside_files() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, source.join("machines")).unwrap();
    assert!(channel::import_stable(&source, &temp.path().join("beta")).is_err());
    fs::remove_file(source.join("machines")).unwrap();
    channel::prepare(&source).unwrap();
    let log = outside.join("untouched");
    fs::write(&log, b"outside").unwrap();
    symlink(&log, source.join("port_forward_tui.background.log")).unwrap();
    assert!(channel::prepare(&source).is_err());
    assert_eq!(fs::read(&log).unwrap(), b"outside");
}
