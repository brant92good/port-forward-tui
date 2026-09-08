# Port Forward TUI

**Open an app running on a remote computer in your own browser. Save the connection and bring it back with your keyboard.**

For example: your development server runs on a remote computer at port **8000**.
Enter `8000` here, then open **http://localhost:8000** on your laptop.
The connection keeps working when you close Windows Terminal.

![The connection manager with saved web app and notebook connections](docs/screenshots/connections.svg)

*Actual app screenshot with simulated example data. The ON rows illustrate the interface; this screenshot did not connect to a server.*

## Is this the project I need?

| I want to… | Start here |
| --- | --- |
| Open remote web apps, notebooks or dashboards through local ports | **This repository.** It works on its own. |
| Also get a remote terminal, return shortcuts and a two-tab desktop button | [Terminal Workspace](https://github.com/brant92good/terminal-workspace), which includes this app |
| Restore my own private SSH settings and agent skills on another laptop | An optional private personal-setup repository above Terminal Workspace |

**Windows 10/11 only.** You need a remote computer you can already reach using
SSH. This app does not rent a server, deploy your code, or start the remote web
app. It supports local TCP forwarding through Windows OpenSSH.

## New to ports or SSH?

- **Port:** the number in an app address. In `http://localhost:8000`, it is `8000`.
- **Remote computer:** the server or workstation where your app is running.
- **Local / this computer:** the Windows computer where you want to open it.
- **SSH:** the login connection used to reach your remote computer securely.
- **Forward / tunnel:** the connection that carries traffic from a port here to
  a port on the remote computer. A **favorite** saves its name and port numbers.
- **TUI:** a text-based app inside a terminal. All its controls work by keyboard.

An AI coding agent can help with setup. You still need the server's address,
your login name, and permission to connect to it. Your agent should ask for
missing details rather than guess them.

## Set up

Run these commands in a **PowerShell tab inside Windows Terminal**, on Windows.

| Needed | How to get it / check it |
| --- | --- |
| Windows Terminal | Install it from Microsoft Store. |
| Git | Install [Git for Windows](https://git-scm.com/downloads/win); `git --version` should work. |
| Windows Python 3.12 or newer | Install [Python](https://www.python.org/downloads/windows/), or use an existing compatible Conda installation. |
| Windows OpenSSH Client | Windows Settings → Optional features → OpenSSH Client. `ssh -V` should work. |
| A working SSH login | Follow the short walkthrough below if you do not already have one. |

**1. Confirm your remote login.** If you normally run `ssh workbox`, then
`workbox` is your **SSH name**. Examples here use that name; replace it with yours.

```powershell
ssh workbox
```

If you have an address and username instead, try `ssh yourname@server-address`.
The first connection may ask you to confirm the server's identity; verify it
with its owner. Type `exit` when you have confirmed that login works.

For a short name, you can add a block to `%USERPROFILE%\.ssh\config`
(preserve any existing entries):

```sshconfig
Host workbox
    HostName server-address
    User yourname
```

Background connections cannot ask you for a password. Set up an SSH key, and
load a passphrase-protected key into Windows `ssh-agent` if needed. Microsoft's
[Windows SSH key guide](https://learn.microsoft.com/en-us/windows-server/administration/openssh/openssh_keymanagement)
walks through that step. Never paste a private key into an issue or an agent chat.

**2. Download and install.** Choose a folder you will keep; shortcuts point to it.

```powershell
git clone https://github.com/brant92good/port-forward-tui.git
cd port-forward-tui
.\install.ps1 -HostName workbox
.\open.ps1
```

You can also run `.\install.ps1` and answer its SSH-name question. It reuses
your saved name on later runs. Setup installs packages in a private `.venv`
folder and adds an entry to the Terminal dropdown. It does not open SSH or start
your saved connections. Global Python packages, PATH and other Terminal profiles
are preserved.

For a particular Python installation:
`.\install.ps1 -HostName workbox -Python 'C:\path\to\python.exe'`.
Conda activation is only needed to select an environment for setup; shortcuts
then call the private Python directly. Keep the base Python installed.

If Windows blocks the script, inspect it first and, where your organization's
policy permits, use `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -HostName workbox`.
This sets policy for that invocation, not for the whole computer.

## Open your first remote app

1. Make sure the app is running on the **remote** computer. Suppose it uses port 8000.
2. In Port Forward TUI, press **A**. Enter `8000` in the remote-port field.
3. Leave the port on this computer blank to use the same number, then press **Enter**.
4. When the row shows **ON**, press **B** to open the local HTTP address in your browser.

![The add form explains remote and local ports and defaults to the same number](docs/screenshots/add-connection.svg)

*Actual add form, captured with example data. Tab moves between fields.*

Prefer fewer keystrokes? Just type **`8000` then Enter** in the main screen.
If port 8000 is already used here, type **`18000:8000`** instead: this computer
uses 18000, the remote app still uses 8000, and the browser address becomes
`http://localhost:18000`. A name can follow: `18000:8000 My web app`.

**ON means the SSH connection is listening on this computer.** The remote app
must also be running. The browser shortcut uses HTTP; for HTTPS, a database,
or another protocol, use the appropriate URL or client yourself.

| Key | What it does |
| --- | --- |
| A | Add using a form; Enter saves and connects |
| Type a number, then Enter | Save and connect using the same port on both computers |
| Up / Down, then Enter or Space | Select a favorite and start or stop it |
| E / D | Edit / delete the selected favorite |
| B / R | Open its HTTP address / restart its connection |
| N / Escape | Jump to quick entry / return to the list |
| Q | Close this screen; background connections continue |
| S | Stop all connections; favorites stay saved |
| F2 / ? | Shortcut settings / help |

Starter favorites are examples and begin OFF. Edit or delete them freely.
Multiple open screens share favorites and connection state. Reboot, sign-out,
or a lost SSH connection ends running tunnels; favorites remain saved.

## Something did not work?

```powershell
.\doctor.ps1
```

This prints local setup checks with next steps. It does not change settings or
try to log in remotely. Python must be available to run the checks.

| What you see | What to do next |
| --- | --- |
| No usable Python | Install Windows Python 3.12+, or pass its real executable with `-Python`. WSL Python cannot run this app. |
| Existing environment is invalid | Restore its base Python, or rename `.venv` as a backup and rerun installation. |
| Permission denied from SSH | Try `ssh YOUR_SSH_NAME` in PowerShell. Check the username, SSH key, and key agent. |
| Server name cannot be found / timeout | Check the SSH name, network, and any VPN the server requires. |
| Local port already in use | Press E and change the port on **this computer**; keep the remote app's port. |
| ON but the page will not open | Confirm the remote app is running and using the expected port/protocol. |

Only this computer can use the local listener (`127.0.0.1`). This does not
publish your development app to the internet. Favorites live in
`%LOCALAPPDATA%\PortForwardTUI\forwards.json`; keep that folder private.

## Use it with a coding agent or script

The same connections can be managed without opening the screen:

```powershell
.\doctor.ps1 --json
.\ports.ps1 list --json
.\ports.ps1 save --remote 8000 --name 'My web app' --json
# Copy the returned id into the next command:
.\ports.ps1 start FAVORITE_ID --json
.\ports.ps1 stop FAVORITE_ID --json
.\ports.ps1 delete FAVORITE_ID --yes --json
```

`save` keeps a stopped connection stopped. It can start the local background
manager, which serializes edits from all screens and agents. Saving an existing
mapping reuses its ID; a name change to an active favorite may restart it.
`--local 18000` chooses a different port here. `start` waits up to five seconds
for a listener; inspect the returned state, since a slow connection can still
be `CONNECTING`. `--wait 0` returns immediately. `stop-all` affects every
connection in the selected data folder.

Results contain `schema_version: 1` and `ok`. Exit codes are **0** for success,
**1** for a failed check/operation, and **2** for invalid arguments. `list` and
`doctor` do not create favorites or start the manager. `UNKNOWN` means live
status was not available; it does not mean the tunnel has stopped.
Use `--data-dir 'C:\path\to\separate-data'` for a separate set of connections.
JSON can contain private host and favorite names; review it before sharing.

An example request for your agent:

> Read AGENTS.md and run doctor.ps1 --json. Explain any missing prerequisites.
> My existing SSH name is YOUR_SSH_NAME. Install with -NonInteractive, then save
> remote port 8000 as My web app. Start that favorite and report its state and
> browser address. Preserve my other favorites and connections.

Replace `YOUR_SSH_NAME` before using that request. Noninteractive setup reports
missing information instead of asking a question in a background process.

## Contribute or explore further

See [AGENTS.md](AGENTS.md) for the code map and test commands. The
[technical reference](REFERENCE.md) covers optional return shortcuts, per-window
focus, foreground mode, persistence details and live SSH verification.
The companion [timing report](https://github.com/brant92good/terminal-workspace/blob/main/docs/before-after.md)
documents measured shortcut performance and its limits.

Screenshots are generated from the running Textual interface by
`scripts/capture_screenshots.py`, using simulated connections and no SSH access.

[MIT license](LICENSE).
