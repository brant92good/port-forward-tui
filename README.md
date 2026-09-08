# Port Forward TUI

Save and manage SSH port forwards from Windows Terminal.

Your web app runs on a remote computer at port `8000`. Enter `8000` here,
then open `http://localhost:8000` on your laptop. Give the connection a name
and reuse it tomorrow. By default, closing the terminal leaves it running.

![Saved web app and notebook connections in Port Forward TUI](docs/screenshots/connections.svg)

*The actual interface, rendered with simulated connections for this screenshot.*

[Install](#set-up) · [First connection](#open-a-remote-app) · [Agent commands](#use-it-with-a-coding-agent-or-script) · [Test evidence](docs/verification.md)

## What it does

- Saves named favorites, so you can start and stop familiar connections with the keyboard.
- Uses the same port on both computers by default. Choose a different local port when one is busy.
- Keeps active forwards in a background process; multiple app tabs share the same connections.
- Provides commands and JSON output for scripts and coding agents.

For **Windows 10/11**, using Windows OpenSSH. A *port forward* carries traffic
from a port on this computer to a port on the remote computer over SSH.
The remote app must already be running. Connections are local to this computer;
this does not publish your app to the internet.

## Set up

You need **Windows Terminal, Git, Windows Python 3.12+, OpenSSH Client**, and
an SSH login that works without a password prompt. Use your existing SSH name
in place of `workbox` below. If `ssh workbox` does not work yet, start with the
[first-connection guide](docs/getting-started.md), including prerequisites and SSH keys.

Run in a PowerShell tab:

```powershell
git clone https://github.com/brant92good/port-forward-tui.git
cd port-forward-tui
.\install.ps1 -HostName workbox
.\open.ps1
```

Setup creates a private Python environment and adds Port Forward TUI to the
Terminal dropdown. Keep this folder in place; the profile points to it.
It preserves your other Terminal profiles and global Python packages.
Saved connections start OFF until you start them.

Using Conda or a Python outside PATH? Pass `-Python 'C:\path\python.exe'` to
the installer. Shortcuts then use the app's environment directly, without
activating Conda or loading a shell profile. Keep that base Python installed.

## Open a remote app

1. Start your app on the remote computer. This example uses port **8000**.
2. In Ports, press **A**, enter `8000`, and press **Enter**. Leave the local port blank to use 8000 here too.
3. When the row shows **ON**, press **B** to open `http://localhost:8000`.

![Add a connection, with the local port defaulting to the remote port](docs/screenshots/add-connection.svg)

*Actual add form with example data. Tab moves between fields.*

You can also type `8000` and Enter directly in the main screen. If that port is
busy on your laptop, type `18000:8000 My web app`; then use `http://localhost:18000`.

| Key | Action |
| --- | --- |
| A | Add a connection with a form |
| Up / Down, then Enter or Space | Start or stop a saved favorite |
| E / D | Edit / delete the selected favorite |
| B / R | Open its HTTP address / restart the connection |
| Q | Close this screen; background connections continue |
| S | Stop all connections |
| ? | Show help and the full key list |

**ON confirms a local listener, not the health of your remote app.** For HTTPS
or a database, use its appropriate address or client. Reboot, sign-out, or a
lost SSH connection ends running tunnels; favorites remain saved.

## What has been checked

A live SSH check sent traffic through a forward after its initiating process
exited. A separate desktop check closed the entire test Terminal window and
confirmed traffic still passed. Both used temporary connections.

[Windows CI](https://github.com/brant92good/port-forward-tui/actions/workflows/test.yml)
tests Python 3.12, 3.13 and 3.14, including keyboard flows, connection commands
and concurrent edits. Desktop persistence was checked on one Windows 11 PC;
it is not covered by every hosted runner. See [test details and reproduction](docs/verification.md).

## Need help?

Run `.\doctor.ps1` for local checks and next steps. It does not change settings
or try to log in. A usable Windows Python is required to run it.

If a row is ON but the page fails, first check the remote app and its port.
For SSH errors, try your SSH login in PowerShell. The
[setup and troubleshooting guide](docs/getting-started.md#something-did-not-work)
covers missing tools, keys, VPNs, port conflicts and blocked PowerShell scripts.

## Use it with a coding agent or script

```powershell
.\doctor.ps1 --json
.\ports.ps1 save --remote 8000 --name 'My web app' --json
.\ports.ps1 start FAVORITE_ID --json   # Use the id returned by save
```

`save` records the favorite; `start` opens it. Inspect the returned state:
a slow SSH connection may still be `CONNECTING`. The
[command guide](docs/automation.md) covers listing, stopping, deleting,
exit codes and JSON output. [AGENTS.md](AGENTS.md) explains how to work on the code.

You can give an agent this request, replacing `YOUR_SSH_NAME`:

> Read AGENTS.md and check setup with doctor.ps1 --json. Use my existing SSH
> name YOUR_SSH_NAME. Install with -NonInteractive, save remote port 8000 as
> My web app, then start it. Report its state and browser address. Preserve
> my other favorites and connections.

## Related tools and development

For one temporary forward, a plain `ssh -L` command may be enough. This app
adds saved names, keyboard controls and shared background connections.
It currently uses one SSH destination per data folder and supports local TCP
forwards. Password-only logins, UDP and reverse forwarding are outside its scope.

[Terminal Workspace](https://github.com/brant92good/terminal-workspace) adds a
Herdr remote-terminal tab, a two-tab Start/desktop button, and return shortcuts.
You can install this port app independently.

See the [technical reference](REFERENCE.md) for optional focus shortcuts,
separate data folders and lifecycle details. Screenshots are generated by
[capture_screenshots.py](scripts/capture_screenshots.py) without SSH access.

[MIT license](LICENSE) · [Report a problem](https://github.com/brant92good/port-forward-tui/issues)
