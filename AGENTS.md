# Working on Ports

Ports saves and controls local SSH forwards across multiple machines. The
production entry point is the compiled `ports` command on Windows, Linux and
macOS (beta). Read README.md and docs/verification.md before changing claims.

## Inspect before changing

Use `ports doctor --json`, `ports machines list --json` and `ports list --json`.
These are read-only: no SSH login, settings writes or daemon startup. Use exact
machine/favorite IDs from results. UNKNOWN means state was not observed; ON
means an owned local SSH listener, not a healthy remote web app.

Installation downloads compiled binaries and does not require a host, Python,
Cargo or Git. OpenSSH is the transport. Follow the README installer commands;
add/import machines afterward. The old Python source and PowerShell entry points
remain for migration compatibility; use the compiled command for native work.
See docs/automation.md for schema version 1 and exit codes 0/1/2.

## Invariants

- Keep saved schemas, IDs, per-device paths and protocol-1 compatibility.
- Preserve concurrent edits through the controller; stale edits/deletes must fail.
- A/quick entry save and connect. E saves; active edits restart the changed mapping.
- E also edits Open automatically. Checkbox-only changes never call the
  controller. A partial field/options save must be reported honestly; preserve
  concurrent edits and the user's input on failure.
- T alone requests a bounded local HTTP title. U changes only this view's label;
  saved names and JSON stay unchanged. Never probe pages automatically.
- OFF stays off on refresh. Only a genuinely new TUI may apply a saved
  `open_automatically` opt-in; absence means false. Stop, delete and stop-all
  cancel pending retries; Stop/Q cancel unsent new-view work. Read-only commands,
  metadata edits, and focusing an existing view never apply this preference.
- Keep automatic-opening metadata in `forward-options.json`, outside the
  protocol-1 `forwards.json` schema. Invalid options fail closed for that
  machine without blocking manual Stop. Preserve unrelated option IDs.
- Multiple background views share state; closing views does not stop forwards.
- Foreground mode owns a single machine and ends its forwards when it closes.
- S stops all listed machines; CLI stop-all requires a machine when ambiguous.
- Keep strict host-key checking, owned process cleanup, and loopback-only binds.
- Read SSH aliases statically; do not execute config commands during discovery.
- Never infer identity from an adjacent tab, duplicate title or a row number.
- Never use personal favorites/keys/hosts in tests, kill unrelated processes,
  steal desktop focus, or publish endpoint.json and private connection metadata.

## Source and checks

See docs/reference.md for the module map. Application code is in src/, tests in
tests/, Windows helpers in native/, setup/release tools in scripts/.

Run `cargo test --locked --all-targets` and
`cargo clippy --locked --all-targets --all-features -- -D warnings`. ConPTY tests own their
pseudo-terminal and do not open desktop windows. Real SSH checks use disposable
fixtures and explicit cleanup. Keep behavioral evidence distinct from compilation,
simulated states, old Python tests and actual released-download verification.

Run `cargo run --locked --features screenshots --example capture` for actual
widget images with synthetic data. License collection/package tools are developer
tools; normal installs need no Python. Check `scripts/collect_licenses.py --check`
and `scripts/check_package_notices.py` when changing dependencies or bundle shape.

Build helpers only into artifact directories. The checkout may also be used by
an existing installation; do not overwrite live launchers during development.
Publish a leaf's qualified release before advancing integration/private pins.
Use the user's authorized publishing mechanism; honor any local instructions
about running git/gh outside a sandbox. Do not add personal values to this public
repository.
