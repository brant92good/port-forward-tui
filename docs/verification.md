# Verification

These are the checks behind the behavior described in the README. Results
below were recorded on September 8, 2026. They establish behavior under the
listed conditions; they do not establish adoption or compatibility with every PC.

## Automated checks

The multi-machine update passed **71 local app tests** and the
[Windows Python 3.12–3.14 CI matrix](https://github.com/brant92good/port-forward-tui/actions/runs/34224908603).
The final picker appearance and import-selection checks also passed their
12-test subset. Machine checks cover first launch without a host, manual add,
keyboard import, Include/cycle handling without command execution, concurrent
creation, legacy-file preservation, separate real local controllers, and
explicit machine selection for ambiguous CLI writes.

The actual noninteractive installer also completed with empty temporary app
data, no host argument and profile registration disabled. It created no machine
or connection. A machine-selected CLI forward with an explicit SSH login port
carried real SSH traffic after the command exited; its test controller and
forward were then stopped. Existing user favorites remained unchanged.

Real Terminal checks verified routing between two machine profiles, including
mixed-machine tabs. Both profiles reached the same physical SSH endpoint; this
tests machine identity/routing, not availability of a second server. See the
[companion desktop checks](https://github.com/brant92good/terminal-workspace/blob/main/docs/verification.md).

Earlier release baseline:

The [v0.4.0 CI run](https://github.com/brant92good/port-forward-tui/actions/runs/34201844088)
completed successfully on Windows with Python 3.12, 3.13 and 3.14. The suite has
58 test cases covering forms, favorites, CLI operations, malformed input,
authenticated local requests, concurrent edits and process ownership.
Hosted runners that deny process-job breakaway skip the desktop detachment
test. A passing CI run alone does not prove terminal-close persistence.

From an installed checkout:

```powershell
.\.venv\Scripts\python.exe -E -s -m unittest discover -v
```

[Current CI](https://github.com/brant92good/port-forward-tui/actions/workflows/test.yml),
[keyboard tests](../tests/test_app.py), [command tests](../tests/test_ports.py) and
[background tests](../tests/test_background.py) are available for inspection.

## Real SSH and terminal closure

On a Windows 11 desktop, a temporary SSH forward carried traffic after the
process that started it exited. A new client found the same connection, and
explicit stop released the port. Repeat with a working SSH name whose server
has SSH listening on remote loopback port 22:

```powershell
.\.venv\Scripts\python.exe -E -s scripts/check_live.py --host workbox
```

[check_live.py](../scripts/check_live.py) uses a temporary data folder and cleans up
its own connection. It connects to your server, so this is an opt-in check.

A separate Windows Terminal check opened an isolated window, started a forward
through the app's actual keyboard control, closed the whole window, and read an
SSH banner through the surviving connection. Its source and run instructions
are in [Terminal Workspace](https://github.com/brant92good/terminal-workspace/blob/main/docs/verification.md).
The CLI was also checked through save, start, traffic after command exit, stop
and delete using an isolated temporary connection.

## Environment and limits

The desktop used Windows 11 Pro build 26200, Windows Terminal 1.24.11911.0,
PowerShell 7.6.5 and Python 3.12.11 selected from Miniforge. Python bootstrap
tests additionally exercise a path with spaces, Unicode and an apostrophe,
a minimal PATH and conflicting Python environment variables.

Windows 10 is a supported target, but this desktop run was on Windows 11.
Another physical laptop and other developers' installations have not yet been
verified. Corporate process restrictions, SSH keys, VPNs and server-side
settings can affect setup or connections.

The app does not restart saved connections after reboot. An ON row confirms
the local SSH listener; a successful request to the remote app is a separate
check. The README screenshots use simulated data and serve as UI examples.

Existing-tab focus timings belong to the
[shortcut benchmark](https://github.com/brant92good/terminal-workspace/blob/main/docs/before-after.md),
which measures returning to an open tab, not TUI startup or SSH establishment.
