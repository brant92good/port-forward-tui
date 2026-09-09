# Port Forward TUI

Manage SSH port forwards across servers in one Windows Terminal list.

Your web app runs on a remote computer at port `8000`. Enter `8000` here,
then open `http://localhost:8000` on your laptop. Give the connection a name
and reuse it tomorrow. Keep forwards to several servers running together.
Closing the terminal leaves them running; interrupted network connections retry
automatically when your Wi-Fi or VPN becomes available again.

![Saved web app and notebook connections in Port Forward TUI](docs/screenshots/connections.svg)

*Example connections.*

[Install](#set-up) · [First connection](#open-a-remote-app) · [Agent commands](#use-it-with-a-coding-agent-or-script) · [Test evidence](docs/verification.md)

## What it does

- Saves named favorites, so you can start and stop familiar connections with the keyboard.
- Adds machines manually or imports SSH names, with separate favorites for each machine.
- Shows every server's favorites together, grouped by server, with live connection state.
- Uses the same port on both computers by default. Choose a different local port when one is busy.
- Keeps active forwards in a background process; multiple app tabs share the same connections.
- Retries network interruptions automatically; stopping a connection cancels its retries.
- Provides commands and JSON output for scripts and coding agents.

For **Windows 10/11**, using Windows OpenSSH. A *port forward* carries traffic
from a port on this computer to a port on the remote computer over SSH.
Start the remote app first, then forward its port to `localhost`.

## Set up

You need **Windows Terminal, Git, Windows Python 3.12+ and OpenSSH Client**.
You can install before choosing a machine. To start forwards later, you need
an SSH login that works without a password prompt; the
[first-connection guide](docs/getting-started.md) explains prerequisites and SSH keys.

Run in a PowerShell tab:

```powershell
git clone https://github.com/brant92good/port-forward-tui.git
cd port-forward-tui
.\install.ps1
.\open.ps1
```

Setup creates an app Python environment and adds Port Forward TUI to the
Terminal dropdown. Keep this folder in place; the profile points to it.
It preserves your other Terminal profiles and global Python packages.
Saved connections start OFF until you start them.

On first launch, **A** adds a machine or **I** imports names from your SSH config.
Select a machine with Enter. The main list shows **all saved servers**; the
selected row chooses where new connections go. **Esc → H** adds/imports machines
or selects another server. Existing background forwards keep running. Herdr is not required.
The [machine guide](docs/machines.md) explains import, manual login settings and
using multiple machines at once.

Using Conda or a Python outside PATH? Pass `-Python 'C:\path\python.exe'` to
the installer. Shortcuts then use the app's environment directly, without
activating Conda or loading a shell profile. Keep that base Python installed.

## Open a remote app

1. Start your app on the remote computer. This example uses port **8000**.
2. From the saved list, press **A**, enter `8000`, and press **Enter**. If you are typing in quick entry, press **Esc** first. Leave the local port blank to use 8000 here too.
3. When the row shows **ON**, press **B** to open `http://localhost:8000`.

![Add a connection, with the local port defaulting to the remote port](docs/screenshots/add-connection.svg)

*Press Esc → A to open this add form. The example has remote port 8000 and
name “My web app”; the blank local port also uses 8000. Tab moves between fields.*

**N** focuses the one-line quick-entry box. **A** opens this form to add a
connection. **E** opens a similar form with a saved connection's values to edit it.

You can also type `8000` and Enter directly in the main screen. If that port is
busy on your laptop, type `18000:8000 My web app`; then use `http://localhost:18000`.

| Key | Action |
| --- | --- |
| A | Add a connection with a form |
| H | Add/import machines or select another server |
| Up / Down, then Enter or Space | Start or stop a saved favorite |
| E / D | Edit / delete the selected favorite |
| B / R | Open its HTTP address / restart the connection |
| Q | Close this screen; background connections continue |
| S | Stop all servers' connections and cancel their retries |
| ? | Show help and the full key list |

**ON confirms a local listener, not the health of your remote app.** For HTTPS
or a database, use its appropriate address or client. Reboot or sign-out ends
running tunnels; favorites remain saved and start OFF next time.

## Work with several servers

The **SERVER** column keeps each server's favorites together. Use Up/Down to
select one, then Enter to start or stop it. You can leave a development server,
notebook server and other machines connected at the same time.

Quick entry and the A form use the selected row's server, shown beside **Add to**
above the list. Even a server with no favorites has a selectable row. H opens
machine management; it does not stop other servers. S stops connections across
the whole list. `--foreground` keeps its existing single-machine behavior.

Two servers can both run their app on remote port 8000, but need different ports
on your laptop: use `8000` for the first and `18000:8000` for the second. If a
local port is already occupied, the app reports the conflict; it does not take
over another process's connection.

## When a laptop loses its connection

A forward you started moves to **RETRYING** after SSH detects a broken network
connection. The background manager waits 2, 4, 8, 16, then at most 30 seconds
between failed attempts. When the network and SSH server are reachable, it
reconnects using the same saved ports, even with the TUI closed. Detection and
SSH login also take time, so recovery is not instant.

**Enter stops retries**, **R retries now**, and **S stops everything**. Connections
you left OFF stay OFF. A successful connection must remain stable for 30 seconds
before the retry delay resets. Authentication failures, changed/untrusted host
keys and occupied local ports show **ERROR** and require your attention.
The app keeps strict SSH host-key checking enabled.

The forward is recreated after a drop; a browser may need refreshing and a
database client may need reconnecting. Reboot, sign-out and a terminated
background manager are not network interruptions and do not restore active state.

## Updating

After updating the code and rerunning installation, close older TUI views and
open a fresh one. To load reconnection support into an already-running manager:

```powershell
.\ports.ps1 machines list --json
.\ports.ps1 restart-manager --machine YOUR_MACHINE_ID --json
```

This briefly interrupts that machine's forwards, then restores only connections
that were ON, connecting or retrying. OFF favorites stay OFF. Repeat for each
running machine; other machines are left alone. Parent setups should use their
normal sync/install commands to follow the versions recorded in their submodules.

## What has been checked

Two independent SSH forwards carried traffic in one overview. A controlled
transport interruption on one recovered automatically while the other remained
usable, including after closing the overview. Stopping it while offline canceled
reconnection. This used two profiles of one physical server; actual laptop
sleep/resume and Wi-Fi roaming remain untested. The
[verification notes](docs/verification.md#combined-server-list-and-network-recovery--september-9-2026)
include the test and reproduction command.

A live SSH check sent traffic through a forward after its initiating process
exited. A separate desktop check closed the entire test Terminal window and
confirmed traffic still passed. Both used temporary connections.

[Windows CI](https://github.com/brant92good/port-forward-tui/actions/workflows/test.yml)
tests Python 3.12, 3.13 and 3.14, including keyboard flows, connection commands
and concurrent edits. Desktop persistence was checked on one Windows 11 PC;
it is not covered by every hosted runner. See [test details and reproduction](docs/verification.md).

## Need help?

Run `.\doctor.ps1` for setup checks and suggested fixes.

If a row is ON but the page fails, first check the remote app and its port.
For SSH errors, try your SSH login in PowerShell. The
[setup and troubleshooting guide](docs/getting-started.md#something-did-not-work)
covers missing tools, keys, VPNs, port conflicts and blocked PowerShell scripts.

## Use it with a coding agent or script

```powershell
.\doctor.ps1 --json
.\ports.ps1 machines add workbox --json
.\ports.ps1 save --machine workbox --remote 8000 --name 'My web app' --json
.\ports.ps1 start FAVORITE_ID --machine workbox --json   # Use the id returned by save
```

`save` records the favorite; `start` opens it. Inspect the returned state:
a slow SSH connection may still be `CONNECTING` or `RETRYING`. The
[command guide](docs/automation.md) covers listing, stopping, deleting,
exit codes and JSON output. [AGENTS.md](AGENTS.md) explains how to work on the code.

You can give an agent this request, replacing `YOUR_SSH_NAME`:

> Read AGENTS.md and check setup with doctor.ps1 --json. Use my existing SSH
> name YOUR_SSH_NAME. Install with -NonInteractive, add that machine, then save
> remote port 8000 as My web app using its machine id. Start it and report its state and browser address. Preserve
> my other favorites and connections.

## Related tools and development

For one temporary forward, a plain `ssh -L` command may be enough. This app
adds saved names, keyboard controls and shared background connections.
Each saved machine has its own data folder and supports local TCP forwards.
Password-only background logins, UDP and reverse forwarding are outside its scope.

[Terminal Workspace](https://github.com/brant92good/terminal-workspace) adds a
remote SSH or Herdr tab, a Start/desktop button, return shortcuts and an optional
third tab for local Herdr.
You can install this port app independently.

See the [technical reference](docs/reference.md) for optional focus shortcuts,
separate data folders and lifecycle details. Screenshots are generated by
[capture_screenshots.py](scripts/capture_screenshots.py) using example data.

Application code is in `port_forward_tui/`, automated checks in `tests/`,
Windows focus code in `native/`, and setup/live-check utilities in `scripts/`.
The root `app.py` and `ports.py` keep the existing launch commands working.

[MIT license](LICENSE) · [Report a problem](https://github.com/brant92good/port-forward-tui/issues)
