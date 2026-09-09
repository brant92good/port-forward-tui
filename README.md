<img src="docs/brand/mark.svg" width="112" align="right" alt="Port Forward TUI logo">

# Port Forward TUI

Your remote dev server, on localhost.

Your app runs on another computer at port **8000**. Type **8000** here, press
**Enter**, and open `http://localhost:8000` on your laptop. Save a name for next
time and keep connections to several servers in one list.

[![Checks](https://github.com/brant92good/port-forward-tui/actions/workflows/test.yml/badge.svg)](https://github.com/brant92good/port-forward-tui/actions/workflows/test.yml)
[![Windows](https://img.shields.io/badge/platform-Windows-65d6be)](docs/verification.md)
[![MIT license](https://img.shields.io/badge/license-MIT-65d6be)](LICENSE)

[Install](#set-up) · [First connection](#open-a-remote-app) · [Agent commands](#use-it-with-a-coding-agent-or-script) · [Report a problem](https://github.com/brant92good/port-forward-tui/issues)

![Saved connections grouped by server in Port Forward TUI](docs/screenshots/connections.svg)

*The actual app with example servers and simulated connection states.*

## Set up

**Want the apps and Python installed for you?**
[Terminal Workspace](https://github.com/brant92good/terminal-workspace#set-up-on-windows)
has a one-command Windows installer. It includes this app, a remote shell tab,
an SSH picker and return shortcuts.

**Only want Port Forward TUI?** Use Windows 10/11 with **Windows Terminal,
Git, Windows Python 3.12+ and OpenSSH Client**. Run in PowerShell:

```powershell
git clone https://github.com/brant92good/port-forward-tui.git
cd port-forward-tui
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\open.ps1
```

The standalone installer creates an app Python environment and adds an entry
to the Terminal dropdown. Keep the checkout in place; the entry points to it.
It doesn't ask for a server. On first launch, **A** adds a machine or **I**
imports SSH names. Select one and press Enter.

Background forwards need an SSH login that works without a password prompt,
usually through a key or key agent. [First-time setup and troubleshooting](docs/getting-started.md)
covers that, missing tools, Conda and custom Python paths. This app currently
supports Windows.

## Open a remote app

1. Start the app on your remote machine. This example uses port **8000**.
2. Type **`8000`** in the quick-entry field and press **Enter**.
3. Select the connection. When it shows **ON**, press **B** to open its local HTTP address.

Need a different port on your laptop? Type **`18000:8000 My web app`**, then
use `http://localhost:18000`. Both sides use the same port when you enter just one.

Prefer separate fields? Press **Esc, then A** to open the add form:

![The add form: remote port 8000, optional local port, and a saved name](docs/screenshots/add-connection.svg)

*This is the A form. N focuses quick entry; E edits a saved connection.
A blank local port uses the remote port. Tab moves between fields.*

## Who is this for?

- You open the same remote web apps or notebooks often and want to save their ports.
- You use several servers and want to see which connections are running together.
- You want to close a terminal window without losing access to a remote app.
- You build with a coding agent and want it to manage forwards through commands you can inspect.

## Saved connections, a few keys

| Key | Action |
| --- | --- |
| Up / Down, then Enter or Space | Start or stop a saved connection |
| N / A | Quick entry / add form |
| E / D | Edit / delete the selected favorite |
| B / R | Open its HTTP address / restart it |
| H | Add, import or choose machines |
| Q | Close the view; background connections continue |
| S | Stop every listed server's connections and pending retries |
| ? | Full keyboard guide |

Press **Esc** first if you're typing in quick entry. **Add to** shows which
server receives a new connection. All servers' favorites stay in the list,
and more than one can be connected at a time. Two remote apps using port 8000
need different local ports, such as 8000 and 18000.
[Machine management and SSH import](docs/machines.md).

## When a laptop loses its connection

Started forwards retry recoverable network interruptions in the background,
even with the view closed. **Enter** stops retries; **R** retries now.
Authentication, host-key and local-port errors need attention.

Closing a tab or the entire Terminal app leaves forwards running. Reboot or
sign-out ends them; saved favorites remain. **ON** means a local listener is
ready—the remote app still needs to be running.
[Recovery timing, limitations and updating a running manager](docs/recovery.md).

### Updating

After updating the app, reopen its views and explicitly restart each running
manager to load the new code. [Update steps](docs/recovery.md#updating) explain
the brief interruption and how requested connections are restored.

## Use it with a coding agent or script

```powershell
.\doctor.ps1 --json
.\ports.ps1 machines list --json
.\ports.ps1 save --machine MACHINE_ID --remote 8000 --name 'My web app' --json
.\ports.ps1 start FAVORITE_ID --machine MACHINE_ID --json
```

Get the machine ID from `machines list` and the favorite ID from `save`.
Saving records a favorite; starting opens its connection. The result reports
whether it is ON, CONNECTING, RETRYING or needs attention.
[Command guide](docs/automation.md) covers adding machines, stopping, deleting
and JSON output. [AGENTS.md](AGENTS.md) maps the code and checks.

## What has been checked

Live checks sent traffic through two SSH forwards together, interrupted one,
and observed it recover while the other stayed usable. A separate desktop
check closed the entire test Terminal window and confirmed traffic still passed.
CI checks keyboard flows, controller commands and concurrent edits on Windows
with Python 3.12–3.14. [Test details and reproduction](docs/verification.md).

The recovery test used two profiles of one physical server. Actual laptop
sleep/resume and Wi-Fi roaming still need testing.

## Need a hand?

Run **`.\doctor.ps1`** for setup checks. If a row is ON but the page won't load,
check that the remote app is running on the expected port. For SSH errors,
try your usual SSH command in PowerShell.
[Troubleshooting](docs/getting-started.md#something-did-not-work).

For one temporary connection, `ssh -L` may be all you need. This app adds saved
names, keyboard controls and shared background connections. It handles local
TCP forwards; reverse forwarding and UDP are outside its scope.

[Technical reference](docs/reference.md) · [Screenshots from the app](scripts/capture_screenshots.py) · [MIT license](LICENSE)
