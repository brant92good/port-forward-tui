# Port Forward TUI

A keyboard-first SSH port-forward manager for **Windows Terminal**.
Save common ports, toggle them with Enter, and close the entire terminal while
your tunnels keep running in the background.

```text
  QUICK FORWARD
  8000  or  18000:8000  [optional name]

  STATE         SAVED FORWARD           LOCAL   ->  REMOTE
  ON            API / dev server        8000    ->  8000
  OFF           Jupyter                 8888    ->  8888

  BACKGROUND ON | Safe to close Terminal. S stops tunnels; Q closes this UI.
```

## Install

Requires **Windows 10/11**, **Python 3.12+**, and the **Windows OpenSSH client**.
Use a host alias already configured in `%USERPROFILE%\.ssh\config` and verify
that key-based login works with `ssh YOUR_HOST` before starting.

```powershell
git clone https://github.com/brant92good/port-forward-tui.git
cd port-forward-tui
.\install.ps1 -HostName workbox
```

Replace `workbox` with your SSH alias. The installer creates a private `.venv`,
sets your target, and adds **Port Forward TUI** to the Windows Terminal dropdown.
Keep the checkout in its installed location because the profile points to it.

If your PowerShell policy prevents running the installer, run the equivalent
commands directly:

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
.\.venv\Scripts\python.exe app.py --host workbox --check
.\.venv\Scripts\python.exe terminal_profile.py
```

Launch from the dropdown or with:

```powershell
.\.venv\Scripts\python.exe app.py
```

For a custom Terminal profile name, use
`.\install.ps1 -HostName workbox -ProfileName "Ports - Workbox"`.
Use `-NoTerminalProfile` to skip registration. For a custom Terminal settings
location, run `terminal_profile.py --settings "C:\path\settings.json"`.
Registration preserves the existing default shell and other profiles, and saves
a backup. For JSONC settings with comments, add a profile manually in Terminal
Settings with this command line (adjust the checkout path):

```text
"C:\path\port-forward-tui\.venv\Scripts\python.exe" "C:\path\port-forward-tui\app.py"
```

## Keyboard

| Key / input | Action |
| --- | --- |
| `8000` then Enter | Save and start local **8000** -> remote **8000** |
| `18000:8000` then Enter | Save and start local **18000** -> remote **8000** |
| `8888 Jupyter` then Enter | Give a forward a friendly name |
| Up / Down | Select a saved forward |
| Enter / Space | Start or stop the selected forward |
| `N` | Focus quick entry |
| `E` | Edit the name or ports; the local port is selected immediately |
| `D` | Delete a favorite (with confirmation) |
| `R` | Restart the selected forward |
| `B` | Open the selected active local HTTP URL |
| `S` | Explicitly stop all tunnels |
| Escape | Return to the saved list |
| `Q` / Ctrl+Q | Close the UI; background tunnels continue |
| `?` | Show help |

Fresh installs include starter favorites for **3000, 5173, 8000, 8080, 8888,
and 6006**. They initially show OFF; selecting one and pressing Enter starts it.
Delete or edit them freely. An intentionally empty list stays empty.

## Background persistence

**Enabled by default.** Starting a forward launches it under a detached,
per-user background supervisor. Closing the tab, closing its window, quitting
Windows Terminal, or a TUI crash does not stop those SSH connections. Reopening
the app reconnects to the same supervisor and displays the active forwards.

To stop a tunnel, select it and press Enter. `S` stops all tunnels. From a shell:

```powershell
.\.venv\Scripts\python.exe app.py --stop-all
.\.venv\Scripts\python.exe app.py --stop-daemon
```

The second command also exits the background supervisor. A supervisor crash
cleans up its owned SSH processes instead of leaving unmanaged tunnels behind.

Persistence here means **surviving the terminal closing**. Signing out or
rebooting ends the processes; there is no Windows startup task. A lost SSH
connection is reported as an error and can be restarted with Enter or `R`.
Saved favorites remain on disk, but are never automatically started.

For temporary forwards that should stop when the UI closes:

```powershell
.\.venv\Scripts\python.exe app.py --stop-daemon
.\.venv\Scripts\python.exe app.py --foreground
```

## Configuration and SSH

Favorites and the target live in `%LOCALAPPDATA%\PortForwardTUI\forwards.json`.
The `keep_alive` setting defaults to `true`; set it to `false` to prefer
foreground mode. Use `--data-dir PATH` for a separate set of favorites and an
independent supervisor. Host selection is per data directory.

To change targets, stop the supervisor and run `app.py --host NEW_ALIAS`.
Aliases, usernames, ports, jump hosts, and keys are supplied through your normal
OpenSSH configuration. Password-only login is not supported by background
processes; encrypted private keys should be loaded into `ssh-agent`.

Each tunnel binds **127.0.0.1 on your computer** and connects to **127.0.0.1 on
the SSH host**. `ON` means the owned local listener exists; the target service
still needs to be running remotely. These are TCP local forwards, not UDP,
SOCKS proxies, or reverse forwards.

The app uses `ssh -N -T -L`, strict host-key verification, keepalives, and
`ExitOnForwardFailure`. It does not run remote commands. Errors appear below
the selected favorite. The local supervisor accepts bounded JSON messages on
loopback, authenticated with a random token stored in its local data directory.
Treat that directory as private to your Windows account.

## Development

```powershell
.\.venv\Scripts\python.exe -m unittest discover -v
.\.venv\Scripts\python.exe app.py --check
```

Tests cover saved settings, keyboard flows, port conflicts, owned-process
cleanup, background detachment, IPC authentication, and Terminal registration.
They do not require a reachable SSH server. To opt into a real transport check
against an existing SSH alias (whose server listens on remote loopback port 22):

```powershell
.\.venv\Scripts\python.exe check_live.py --host workbox
```

This creates an isolated temporary forward, exits its initiating client,
verifies traffic still crosses it, reconnects a new client, and explicitly
cleans up. Your saved favorites are not changed.

Some managed process environments (including GitHub-hosted Windows runners)
prohibit breaking out of their process job. The app reports that restriction
instead of claiming a tunnel will survive. Use a regular Windows Terminal
session for background mode. CI still tests the real control server; it skips
only the desktop detachment test when the runner explicitly denies breakaway.

Implementation: Python, [Textual](https://textual.textualize.io/), Windows
[process jobs](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
and [OpenSSH local forwarding](https://man.openbsd.org/ssh#L).

## License

[MIT](LICENSE).
