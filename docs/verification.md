# Native verification

Updated September 10, 2026. Native v0.7.1 is a release candidate. Results below
describe their actual fixtures, not every developer's computer.

## Verified locally on Windows

The working native candidate passes **20 tests** and Clippy with warnings denied.
Independent review passed the controller and keyboard behavior changes.

- Actual loopback controller requests exercise competing edits/deletes,
  authentication, slow peers and stop-all with malformed favorites.
- A delayed, fragmented request reproduces Windows error 10053 before the fix
  and succeeds afterward. Accepted Winsock sockets inherited the listener's
  nonblocking mode; worker sockets now explicitly use blocking reads with the
  existing two-second message deadline. No failed mutation is silently retried.
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

All four Unix jobs passed in
[run 34382122697](https://github.com/brant92good/port-forward-tui/actions/runs/34382122697).
On Linux x64 and ARM64, separate real pseudo-terminal hangup and SIGTERM tests
verify that foreground mode releases its owned SSH process group. The Unix
input backend observes these signals even after the terminal stops producing
input. Windows retains its own console input backend.

Both Linux targets also pass `scripts/check_ssh_forwarding.py`: two detached
native controllers carry real HTTP through system OpenSSH. Dropping one SSH
connection triggers retries while the other keeps serving traffic. Restoring
the network gate reconnects without replacing either controller. Stop/delete
cancel retries, saved OFF rows remain OFF, and closing the launching command
does not own the controllers' lifetime. The fixture uses disposable keys,
loopback servers and explicit cleanup, not personal hosts.

The Windows socket inheritance correction passed all five platform jobs at
`3656b4a`. Its immutable v0.7.0 tag then exposed an intermittent macOS rebind
failure before assets were published; that tag remains unchanged.

The fixed-sample [macOS diagnostics](https://github.com/brant92good/port-forward-tui/actions/runs/34385711045)
found no failures in 24 isolated samples, but 26 failures with parallel socket
inspection. Both the raw reuse-enabled bind and an ordinary listener returned
EADDRINUSE, then succeeded milliseconds later. Concurrent inspection is
implicated; the precise kernel mechanism remains an inference.

The macOS preflight now allows that specific error up to 250 ms at 5 ms intervals.
It retries only an idempotent bind, with fresh sockets; other errors return
immediately. Windows/Linux keep one attempt. Initial success adds no wait and
genuinely occupied ports remain rejected. The same
[fixed observer matrix](https://github.com/brant92good/port-forward-tui/actions/runs/34386390436)
passes on both macOS architectures, retaining raw-first failures separately from
the corrected product result. All five native targets also passed
[run 34386390341](https://github.com/brant92good/port-forward-tui/actions/runs/34386390341).
The corrected release is v0.7.1.

## Installation evidence

The actual local Windows ZIP passed fresh-install/update checks in an isolated
Unicode/apostrophe path, checksum-failure refusal, non-owned-directory refusal,
saved-data preservation and a polluted Python/Conda environment. No live user
settings or controllers were used. Actual public HTTPS downloads remain a
separate release gate on Windows x64, Linux x64/ARM64 and macOS ARM64/Intel.

## Not established by these checks

- Real OpenSSH transport on Windows and macOS: not covered by the Linux fixture.
- Final published HTTPS installation on all advertised targets: pending release.
- Native Windows desktop focus/taskbar/Explorer behavior: integration-layer
  evidence must be checked separately, using owned test windows.
- Physical laptop sleep, Wi-Fi roaming and enterprise VPN conditions.
- Native macOS desktop use. Keep the beta label until exercised.

[Earlier Python evidence](history/python-verification.md) is historical and must
not be used as proof for the Rust runtime.

[The local CLI measurement](performance.md) compares full Python/native process
startup for a read-only query. It does not measure TUI paint or shortcut focus.
