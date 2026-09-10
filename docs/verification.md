# Native verification

## v0.9.0 candidate: editing, groups, output and title preview

These changes are source-qualified locally; the new five-target release and
actual HTTPS asset gates are pending. Earlier released evidence below remains
historical. macOS remains beta.

- The E form includes the existing Open automatically preference. Metadata-only
  edits do not call the controller or change a current connection. Tests cover
  stale favorites/options, invalid options, separate-file partial failures,
  on/off round trips, new-view behavior and small form navigation.
- Name-first rows sit under nonselectable server headings. Buffer tests exercise
  narrow viewports, Unicode, long names and selected-row visibility; actual
  keyboard scenarios retain their machine association and saved-value checks.
- [Buffered-output measurements](rendering.md) compare identical frame bytes in
  hidden ConPTY sessions. Failure tests ensure a dropped writer cannot replay
  buffered content after leaving the alternate screen. They do not measure
  physical painting or desktop return shortcuts.
- T explicitly previews one ON forward's HTTP HTML title. Tests cover response,
  size, time, encoding and control-text limits; saved names and connection
  state remain unchanged. An actual disposable OpenSSH forward carried the
  controlled HTML response. U is view-only; this is not process discovery,
  authentication, HTTPS or JavaScript execution.

The combined default Windows suite passed 57 active tests, with four deliberate
harness-specific ignores. The separate actual SSH title case also passed.
All-target/all-features Clippy passed after the screenshot feature was added.
Native documentation captures use the actual widget renderers with synthetic
state; no request is made to create the title screenshot.

Four local notice-collector tests pass. An isolated Windows installer fixture
passes full-notice installation/update, explicit 0.8.1 legacy installation,
and rejection of five malformed/tampered bundle variants without changing
installed files. The real 0.9 package, Unix notice installers and public HTTPS
downloads still require their release-matrix checks. Normal use remains compiled;
these Python programs are developer fixtures only.

## v0.8.1 automatic opening

The v0.8.1 release adds per-favorite opt-in preferences. Local
Windows tests cover separate metadata/unchanged legacy schema, concurrent
preference edits, malformed options, frozen destination/favorite checks and
read-only CLI behavior. Actual ConPTY checks use isolated controllers and
owned SSH-shaped loopback processes: new-view starts across two machines,
concurrent views retaining running PIDs, manual Stop surviving refresh,
new-view reapplication, Quit/Stop-all cancellation, retrying port reservations
and foreground-only scope/cleanup. No personal forward or desktop window is
used. Independent review replayed the automatic-opening ConPTY cases, metadata
checks and forced-app-exit cleanup. Additional regressions cover three queued
manual Stops, repeated-key coalescing, persistent per-rule failures, and changed
preferences/destinations while controller preparation is delayed.

The immutable runtime is `78b183021f22b7380faef3334ade9ec50696c474`.
All five native target jobs, publication and five actual public HTTPS
install/update jobs passed in
[run 34476916550](https://github.com/brant92good/port-forward-tui/actions/runs/34476916550).
Windows x64, Linux x64/ARM64 and macOS Intel/Apple Silicon used the advertised
installer and actual release downloads. Windows additionally passed native
new-view automatic opening through an existing historical Python controller,
including simultaneous views retaining the same SSH process and manual Stop.
The Windows ZIP SHA-256 is
`34cdcdc34768785d919d6059e6d757efaa2d481e39babf45d375009e9a32e0b8`.

Independent Windows downloads verified the ZIP sidecar and all three bundled
binary hashes. The executable SHA-256 is
`79d26dc35f7342f57ef8ffc1fd1f7b75d71c431dd0b65b6f84dc4cebbc8fb735`.
Actual HTTPS fresh installation and update also passed locally under Windows
PowerShell 5.1 and PowerShell 7.6.6. The saved forwards stayed byte-identical and
an enabled automatic-opening preference survived the update. Metadata edits and
read-only listing created no controller; listing correctly reported UNKNOWN
with `background: not_connected`, rather than inventing an observed OFF state.

The v0.8.0 tag remains immutable and has no assets. Its ARM64 release test
waited for a brief settings-save message that a concurrent automatic completion
could replace. The saved preference and OFF state were correct. v0.8.1 changes
that test to inspect the durable saved value and selected-row detail; application
behavior is unchanged. The original five-platform candidate passed in
[run 34475514796](https://github.com/brant92good/port-forward-tui/actions/runs/34475514796).

The separate historical Python UI suite had one first-attempt form-dismissal
failure in
[run 34476916541](https://github.com/brant92good/port-forward-tui/actions/runs/34476916541).
That UI and test are unchanged from v0.7.3. The exact case passed independently
on Windows and the one hosted failed-job replay passed. The first log only shows
the modal still open; it does not establish whether input delivery or dismissal
timing caused it. This is retained as an unresolved historical UI test flake,
separate from the passing Rust-to-historical-controller compatibility gate.

The earlier v0.7.3 evidence below remains historical; its real Windows Terminal
window-close result does not qualify every new-view preference scenario.

Updated September 10, 2026. Native v0.7.3 binaries and installer checks are published. Results below
describe their actual fixtures, not every developer's computer.

## Windows captured-command correction

A later real-machine check found that the first `save --json` could exit while
its newly detached controller kept the calling script's output pipes open.
The v0.7.1 tests prestarted controllers and missed this specific launch path.
Redirecting the daemon's standard streams did not prevent Windows from inheriting
the original incoming handles as well. Clearing inheritance on the CLI's three
standard handles fixed direct capture, but the packaged PowerShell 5.1 and 7
wrappers exposed additional inherited handles. The current correction uses a
Windows process handle list containing only null stdin and the controller log.
It preserves the caller's environment/current directory and the existing refusal
to detach from a restrictive enclosing job. No shell helper or compiler is added
to normal startup. [Windows handle-list contract](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute).

`tests/native_cli_capture.rs` reproduces the failure without connecting to SSH.
It checks first save and restart: the CLI exits, stdout/stderr reach EOF, stdin
has no inherited reader, and the controller remains alive. It then shuts down
only that fixture controller. The same test fails before the fix and passes
afterward. Cargo and the hosted Windows runner both own restrictive process jobs.
A CI-only hidden worker must prove that its owned test runs outside those jobs;
the product's detachment behavior is unchanged. This is an explicit separate gate,
not a skipped behavior check. Extra inherited handles and both packaged PowerShell
wrapper chains are regression gates too. All passed locally, including an
unrelated inheritable pipe and exact Unicode, spaced and trailing-backslash
paths. The hosted worker's job-free checks and both capture cases passed in
[run 34393598746](https://github.com/brant92good/port-forward-tui/actions/runs/34393598746)
and again in the v0.7.3 release below. A separate actual Windows Terminal check,
described below, now also verifies first-save and window-close behavior.

## Verified locally on Windows

The working native candidate passes **22 tests** (including two ConPTY tests)
and Clippy with warnings denied.
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

## Cross-platform qualification

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

The immutable v0.7.3 runtime is `6c9c3adea0541209657ba21cb2222110fc3ee1bc`.
All five native jobs, release packaging and all five public HTTPS install/update
jobs passed in
[run 34394389900](https://github.com/brant92good/port-forward-tui/actions/runs/34394389900).
Windows, Linux x64/ARM64 and macOS Apple Silicon/Intel used the advertised
installer URL and downloaded the actual published bundles.

The Windows HTTPS test was also repeated locally: fresh installation, update,
checksum refusal, directory ownership, saved favorites, Unicode/quoted paths,
polluted environment and unchanged PATH all passed. The released executable
SHA-256 is `1ef7ba64f8b55dc23d09efcd50ef4f601645288b5144be4ee6085806c2c9a81f`.
An independent download matched both that executable and the published ZIP
digest `89287f47046c3030f135e9055609301c69c872a12cd25d35e5e261ec069edb84`.

Those exact Windows bytes also carried real HTTP200 traffic through an existing
trusted SSH connection using isolated saved data and an ephemeral local port.
The controller stayed alive after the launching CLI exited; stopping released
the port and fixture cleanup completed. This is a local one-computer check,
without desktop activation; it does not test Windows network roaming.

The integration layer separately ran these same released Ports bytes inside a
real Windows Terminal window. The first saved OFF rule started its controller
from inside that window; the CLI exited successfully and its captured streams
reached EOF. Closing the owned window left the same controller responding to
authenticated requests. Cleanup then verified the launching processes had exited
and shut down only that fixture controller. The window measured 701 by 400 pixels,
and the foreground window stayed unchanged before, during and after the test.
This single local check used no SSH connection or keyboard input. Combined with
the separate traffic check above, it covers both real forwarding and actual
Terminal window closure without claiming to test a forwarding connection during
that window-close run. It does not qualify hotkeys, taskbar grouping or every
Windows host's process restrictions.

### Earlier v0.7.1 installation checks

The actual local Windows ZIP passed fresh-install/update checks in an isolated
Unicode/apostrophe path, checksum-failure refusal, non-owned-directory refusal,
saved-data preservation and a polluted Python/Conda environment. No live user
settings or controllers were used.

The immutable v0.7.1 runtime is `ca58e069f923bb108c3e05ef5e9a6e4726606e4e`.
All five native jobs, release packaging, and all five actual public HTTPS
install/update jobs passed in
[run 34387071375](https://github.com/brant92good/port-forward-tui/actions/runs/34387071375).
Targets were Windows x64, Linux x64/ARM64 and macOS ARM64/Intel. The advertised
installer was downloaded over HTTPS, then downloaded the published binary bundle;
the tests did not substitute a local package.

The Windows HTTPS check was repeated locally without changing the user's PATH.
Fresh install, update, invalid checksum rejection, directory ownership, saved
favorites, Unicode/quoted paths and polluted environment all passed. Its executable
SHA-256 was `b03ae622b861c3a76ab383bf26dcf61600bde794106f94fb40105d66c5b7ee5b`.

## Not established by these checks

- Real OpenSSH transport on macOS. The Windows check above covers one existing
  trusted configuration; it is not the disposable Linux multi-host network fixture.
- Native Windows desktop focus/taskbar/Explorer behavior: integration-layer
  evidence must be checked separately, using owned test windows.
- Physical laptop sleep, Wi-Fi roaming and enterprise VPN conditions.
- Native macOS desktop use. Keep the beta label until exercised.

[Earlier Python evidence](history/python-verification.md) is historical and must
not be used as proof for the Rust runtime.

[The local CLI measurement](performance.md) compares full Python/native process
startup for a read-only query. It does not measure TUI paint or shortcut focus.
