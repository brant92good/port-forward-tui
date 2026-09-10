use port_forward_tui::{
    auto_open::{self, Preferences},
    forward_form::{self, Request},
    machines::Catalog,
    store::{Forward, Store},
};
use serde_json::json;
use std::{cell::Cell, fs};

fn request(rule: &Forward, original: Option<bool>, enabled: bool) -> Request {
    Request {
        previous: rule.clone(),
        rule: rule.clone(),
        original_auto: original,
        enabled,
    }
}

#[test]
fn preference_only_save_never_calls_controller_and_preserves_other_machines() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(temp.path()).unwrap();
    let first = catalog.add("one.invalid", "One", None, None).unwrap();
    let second = catalog.add("two.invalid", "Two", None, None).unwrap();
    let rule = Store::load(&first.directory).unwrap().settings.forwards[0].clone();
    let before = fs::read(first.directory.join("forwards.json")).unwrap();
    let other = fs::read(second.directory.join("forwards.json")).unwrap();
    for (old, enabled) in [(false, true), (true, false)] {
        let outcome = forward_form::save(
            &first.directory,
            request(&rule, Some(old), enabled),
            |_, _| panic!("Metadata-only edit called the controller"),
        );
        assert!(outcome.success, "{}", outcome.notice);
        assert_eq!(
            Preferences::load(&first.directory)
                .unwrap()
                .enabled(&rule.id),
            enabled
        );
        let (jobs, warnings) = auto_open::collect(&[first.clone(), second.clone()]);
        assert!(warnings.is_empty());
        assert_eq!(jobs.len(), usize::from(enabled));
        assert!(jobs.iter().all(|job| job.machine.id == first.id));
        assert_eq!(
            fs::read(first.directory.join("forwards.json")).unwrap(),
            before
        );
        assert_eq!(
            fs::read(second.directory.join("forwards.json")).unwrap(),
            other
        );
        for machine in [&first, &second] {
            assert!(!machine.directory.join("endpoint.json").exists());
            assert!(!machine.directory.join("daemon.lock").exists());
        }
    }
}

#[test]
fn combined_save_checks_old_option_before_mutation_and_new_forward_afterward() {
    let temp = tempfile::tempdir().unwrap();
    let machine = Catalog::new(temp.path())
        .unwrap()
        .add("fixture.invalid", "Fixture", None, None)
        .unwrap();
    let mut store = Store::load(&machine.directory).unwrap();
    let rule = store.settings.forwards[0].clone();
    let mut edit = request(&rule, Some(false), true);
    edit.rule.name = "Updated favorite".into();
    let before = fs::read(machine.directory.join("forwards.json")).unwrap();
    auto_open::set(&machine.directory, &rule.id, true).unwrap();
    let outcome = forward_form::save(&machine.directory, edit.clone(), |_, _| {
        panic!("Known stale option must fail before field mutation")
    });
    assert!(!outcome.success);
    assert!(outcome.notice.contains("changed in another view"));
    assert_eq!(
        fs::read(machine.directory.join("forwards.json")).unwrap(),
        before
    );
    auto_open::set(&machine.directory, &rule.id, false).unwrap();
    let called = Cell::new(0);
    let outcome = forward_form::save(&machine.directory, edit.clone(), |updated, expected| {
        called.set(called.get() + 1);
        assert_eq!(expected, &rule);
        store.settings.forwards[0] = updated.clone();
        store.save(store.settings.forwards.clone())?;
        Ok(json!({"ok":true}))
    });
    assert!(outcome.success, "{}", outcome.notice);
    assert_eq!(called.get(), 1);
    assert_eq!(outcome.saved_forward, Some(edit.rule));
    assert!(
        Preferences::load(&machine.directory)
            .unwrap()
            .enabled(&rule.id)
    );
}

#[test]
fn stale_forward_and_partial_metadata_failure_do_not_claim_a_full_save() {
    let temp = tempfile::tempdir().unwrap();
    let machine = Catalog::new(temp.path())
        .unwrap()
        .add("fixture.invalid", "Fixture", None, None)
        .unwrap();
    let mut store = Store::load(&machine.directory).unwrap();
    let rule = store.settings.forwards[0].clone();
    let mut edit = request(&rule, Some(false), true);
    edit.rule.name = "Saved fields".into();
    let changed = Forward {
        name: "Other view".into(),
        ..rule.clone()
    };
    store.settings.forwards[0] = changed;
    store.save(store.settings.forwards.clone()).unwrap();
    let outcome = forward_form::save(&machine.directory, edit.clone(), |_, _| {
        panic!("Stale forward was accepted")
    });
    assert!(!outcome.success);
    assert!(!machine.directory.join(auto_open::FILE).exists());
    store.settings.forwards[0] = rule.clone();
    store.save(store.settings.forwards.clone()).unwrap();
    let outcome = forward_form::save(&machine.directory, edit.clone(), |updated, _| {
        store.settings.forwards[0] = updated.clone();
        store.save(store.settings.forwards.clone())?;
        // An independently changed sidecar between the two owners' writes.
        fs::write(machine.directory.join(auto_open::FILE), "broken")?;
        Ok(json!({"ok":true}))
    });
    assert!(!outcome.success);
    assert!(
        outcome
            .notice
            .contains("Forward changes saved; automatic opening was not saved")
    );
    assert_eq!(outcome.saved_forward, Some(edit.rule.clone()));
    assert_eq!(
        Store::load(&machine.directory).unwrap().settings.forwards[0],
        edit.rule
    );
    assert_eq!(
        fs::read_to_string(machine.directory.join(auto_open::FILE)).unwrap(),
        "broken"
    );
    let unchanged = forward_form::save(
        &machine.directory,
        request(&edit.rule, None, false),
        |_, _| panic!("Unavailable no-change save must not call the controller"),
    );
    assert!(!unchanged.success);
    assert!(
        unchanged
            .notice
            .contains("Automatic opening is unavailable")
    );
    // Unavailable options do not prevent ordinary edits or overwrite that file.
    let mut fields_only = request(&edit.rule, None, false);
    fields_only.rule.name = "Still editable".into();
    let outcome = forward_form::save(&machine.directory, fields_only, |updated, _| {
        store.settings.forwards[0] = updated.clone();
        store.save(store.settings.forwards.clone())?;
        Ok(json!({"ok":true}))
    });
    assert!(outcome.success, "{}", outcome.notice);
    assert!(outcome.notice.contains("Automatic opening is unavailable"));
    assert_eq!(
        fs::read_to_string(machine.directory.join(auto_open::FILE)).unwrap(),
        "broken"
    );
}

#[test]
fn failed_upsert_reply_reports_observed_saved_fields_without_changing_options() {
    let temp = tempfile::tempdir().unwrap();
    let machine = Catalog::new(temp.path())
        .unwrap()
        .add("fixture.invalid", "Fixture", None, None)
        .unwrap();
    let mut store = Store::load(&machine.directory).unwrap();
    let rule = store.settings.forwards[0].clone();
    let mut edit = request(&rule, Some(false), true);
    edit.rule.name = "Committed before error".into();
    let outcome = forward_form::save(&machine.directory, edit.clone(), |updated, _| {
        store.settings.forwards[0] = updated.clone();
        store.save(store.settings.forwards.clone())?;
        anyhow::bail!("Fixture restart failed after saving")
    });
    assert!(!outcome.success);
    assert_eq!(outcome.saved_forward, Some(edit.rule));
    assert!(outcome.notice.contains("Forward changes saved"));
    assert!(outcome.notice.contains("Fixture restart failed"));
    assert!(!machine.directory.join(auto_open::FILE).exists());
}
