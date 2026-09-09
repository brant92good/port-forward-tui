use port_forward_tui::{
    forwarding::{Manager, State},
    process::NativeBackend,
    store::{Forward, Settings},
    views,
};
use std::{
    fs,
    io::Read,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};
fn compile(directory: &Path) -> PathBuf {
    let executable = directory.join(if cfg!(windows) {
        "ssh-fixture.exe"
    } else {
        "ssh-fixture"
    });
    let mut command = Command::new("rustc");
    command
        .arg("--edition=2024")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/ssh_fixture.rs"))
        .arg("-o")
        .arg(&executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    executable
}
fn unused_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
#[track_caller]
fn wait(stage: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "Owned process condition timed out: {stage}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
fn pid(directory: &Path, file: &str) -> Option<u32> {
    fs::read_to_string(directory.join(file))
        .ok()?
        .trim()
        .parse()
        .ok()
}
fn fixture(
    root: &Path,
    name: &str,
    executable: &Path,
) -> (Manager<NativeBackend>, Forward, PathBuf) {
    let path = root.join(name);
    fs::create_dir_all(&path).unwrap();
    let settings = Settings {
        host: format!("{name}.example"),
        ssh_config: Some(path.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let rule = Forward::new(unused_port(), 8000, name).unwrap();
    (
        Manager::new(settings, NativeBackend::with_executable(executable.into())),
        rule,
        path,
    )
}
#[test]
fn actual_owned_listener_proxy_descendants_recovery_and_other_machine_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let executable = compile(temp.path());
    let (mut first, one, path_one) = fixture(temp.path(), "first", &executable);
    let (mut second, two, path_two) = fixture(temp.path(), "second", &executable);
    first.start(&one, Instant::now()).unwrap();
    second.start(&two, Instant::now()).unwrap();
    wait(
        "both original listeners ON and descendants recorded",
        || {
            first.poll(Instant::now());
            second.poll(Instant::now());
            first.state(&one.id) == State::On
                && second.state(&two.id) == State::On
                && pid(&path_one, "child.pid").is_some()
                && pid(&path_two, "child.pid").is_some()
        },
    );
    let first_child = pid(&path_one, "child.pid").unwrap();
    let first_parent = pid(&path_one, "parent.pid").unwrap();
    let second_child = pid(&path_two, "child.pid").unwrap();
    let mut service = TcpStream::connect(("127.0.0.1", one.local_port)).unwrap();
    service
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut reply = [0; 10];
    service.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"fixture-ok");
    drop(service);
    fs::write(path_one.join("exit"), b"fail this owned process").unwrap();
    wait("first connection moves to RETRYING after exit", || {
        first.poll(Instant::now());
        first.state(&one.id) == State::Retrying
    });
    wait("first exited parent and proxy descendant are gone", || {
        !views::process_alive(first_parent) && !views::process_alive(first_child)
    });
    second.poll(Instant::now());
    assert_eq!(second.state(&two.id), State::On);
    assert!(views::process_alive(second_child));
    fs::remove_file(path_one.join("exit")).unwrap();
    fs::remove_file(path_one.join("child.pid")).unwrap();
    wait(
        "first connection recovers with a new proxy descendant",
        || {
            first.poll(Instant::now());
            first.state(&one.id) == State::On
                && pid(&path_one, "child.pid").is_some_and(|id| id != first_child)
        },
    );
    let recovered_child = pid(&path_one, "child.pid").unwrap();
    let recovered_parent = pid(&path_one, "parent.pid").unwrap();
    first.stop(&one.id);
    wait("recovered connection parent and proxy stop", || {
        !views::process_alive(recovered_parent) && !views::process_alive(recovered_child)
    });
    assert_eq!(first.state(&one.id), State::Off);
    second.poll(Instant::now());
    assert_eq!(second.state(&two.id), State::On);
    second.close();
    wait("second connection proxy stops", || {
        !views::process_alive(second_child)
    });
    first.poll(Instant::now() + Duration::from_secs(100));
    assert_eq!(first.state(&one.id), State::Off);
}
