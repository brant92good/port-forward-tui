# First connection

[Install Ports](../README.md#install), then run `ports`. Installation does not ask
for a server. Windows and Linux have native builds; macOS remains beta.

## Prepare an SSH login

If you usually run `ssh workbox`, use `workbox` as the machine's SSH name. An
address also works: `alex@server.example.com`. Open a normal shell and confirm
that login works first:

```sh
ssh workbox
```

For first-time host trust, verify the displayed fingerprint with the server's
owner. Background forwarding uses strict host-key checking and cannot answer
interactive prompts. Key authentication through your existing SSH agent works
with encrypted keys. Exit the remote shell when finished.

`ports doctor` checks for OpenSSH and validates saved metadata without connecting
or installing anything. It does not need Python, Conda, Cargo, or Git.

## Choose a machine

Run `ports`. Press **A** to enter an SSH alias or `user@address`. The optional
SSH login port is normally 22; it is different from the app port you forward.
Alternatively, **I** reads your SSH config and previews its host names. Space
selects names; Enter imports them. [More about import](machines.md).

## Open the remote app locally

A port is the number in an address such as `localhost:8000`. The remote app
listens on a port on the server. Ports creates a local address that reaches it
through SSH.

Start the remote app first. In Ports, enter `8000` and press Enter. That saves
and starts local port 8000 → remote port 8000. Press B on its row to open the
local HTTP address. Use `18000:8000 API` to choose another local port and a name.

**A** opens the add form, **N** focuses quick entry, and **E** edits a favorite.
Tab moves between fields. Adding through the form saves and connects; editing
an active favorite restarts that mapping with the new values.

Want selected favorites each time you open Ports? Select one, press **E**,
Tab to **Open automatically**, toggle it with Space, then press Enter. F2 settings
also has the same option. It starts disabled.
Saving the preference leaves the current connection alone. A new view applies
enabled favorites across listed machines; Stop keeps them off until you start
them yourself or open another new view.

For an ON web app, **T** previews its HTML page title. **U** uses the title only
in this view, and Enter keeps the saved name. It is a manual HTTP check, not
service discovery; it does not follow redirects, authenticate to a web app,
use HTTPS or run JavaScript. Use E to save your own name for any service.

## Something did not work

| Symptom | Next step |
| --- | --- |
| `ports` is not found after installation | Open a new shell, or use the executable path printed by the installer. |
| OpenSSH is missing | Install your OS's OpenSSH client; confirm `ssh -V` works. |
| Login or host-key error | Try the same alias in a normal SSH session and fix authentication/trust there. |
| Local port is occupied | Edit the local port. Ports does not stop another app to take its port. |
| ON but the page does not load | Check the remote app's port and whether it listens on the server's loopback interface. |
| RETRYING | The requested connection is waiting for the network/server. Enter cancels; R retries now. |
| UNKNOWN in CLI output | Live state was not observed. Do not assume that the connection stopped. |
| Background detachment denied on Windows | A surrounding app may restrict child processes. Use a normal Windows Terminal window, or use `--foreground` with its shorter lifetime. |

Your machine's SSH configuration continues to determine keys, jump hosts and
ProxyCommand behavior. Ports does not replace your SSH setup. `--foreground`
owns connections in the current view and stops them when that view ends.

## Update

Run the installer for the new release, then reopen views. Existing controllers
keep their loaded version until you explicitly run:

```sh
ports restart-manager --machine MACHINE_ID --json
```

This briefly stops that machine's requested forwards and restores them with the
new controller. Saved OFF favorites remain stopped. [Details](recovery.md).
