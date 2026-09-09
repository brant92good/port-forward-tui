# Native verification

Updated September 10, 2026. Native v0.7.0 is a release candidate. Results below
describe their actual fixtures, not every developer's computer.

## Verified locally on Windows

The working native candidate passes **18 tests** and Clippy with warnings denied.
Independent review passed the controller and keyboard behavior changes.

- Actual loopback controller requests exercise competing edits/deletes,
  authentication, slow peers and stop-all with malformed favorites.
- An SSH-shaped fixture opens real TCP listeners and spawns a child. A dropped
  parent triggers cleanup/retry while a second machine stays ON. Recovery makes
  new owned processes; explicit stop ends their descendants.
- The actual TUI runs in ConPTY. Tests cover A save-and-connect, quick-entry
  rename retaining its ID, another server's requested-port conflict, S stopping
  both servers, two views, deletion and closing views while controllers continue.
  Controllers start outside ConPTY's restrictive harness job; this does not prove
  every enclosing Windows application allows detached child processes.
- Read-only commands create no files/controllers. String-valued saved ports and
  machine-picker JSON remain compatible.
- `scripts/check_native_compat.py` runs actual native CLI and historical Python
  clients against both native and historical controllers. IDs, OFF state,
  stale-edit rejection and cross-runtime file locks pass. The independent run
  also used the Windows venv launcher, whose PID differs from its daemon child.

Native screenshots use the actual Ratatui list/form widgets with synthetic
machines and states. `cargo run --example capture` reproduces them without SSH.
Headless browser visual inspection and independent A-versus-N review passed.

## Cross-platform qualification in progress

The first matrix exposed a Unix reconnect failure after a used socket entered
TIME_WAIT. `9affbaa` adds a Unix-only reuse-address preflight, matching SSH's
rebind behavior. Its real socket regression and complete owned-process recovery
passed on all four Unix targets in
[run 34379777252](https://github.com/brant92good/port-forward-tui/actions/runs/34379777252).
That run subsequently exposed a keyboard fixture race: S was sent before the
preceding mutation response. The fixture now waits for the updated main-view row.

Unix foreground HUP/TERM cleanup and actual Linux OpenSSH forwarding/recovery
checks are being qualified next. Normal Q cleanup and a mock transport are not
sufficient evidence for terminal-close or real SSH behavior.

## Installation evidence

The actual local Windows ZIP passed fresh-install/update checks in an isolated
Unicode/apostrophe path, checksum-failure refusal, non-owned-directory refusal,
saved-data preservation and a polluted Python/Conda environment. No live user
settings or controllers were used. Actual public HTTPS downloads remain a
separate release gate on Windows x64, Linux x64/ARM64 and macOS ARM64/Intel.

## Not established by these checks

- Real released-binary OpenSSH transport recovery: pending its separate fixture.
- Final published HTTPS installation on all advertised targets: pending release.
- Native Windows desktop focus/taskbar/Explorer behavior: integration-layer
  evidence must be checked separately, using owned test windows.
- Physical laptop sleep, Wi-Fi roaming and enterprise VPN conditions.
- Native macOS desktop use. Keep the beta label until exercised.

[Earlier Python evidence](history/python-verification.md) is historical and must
not be used as proof for the Rust runtime.
