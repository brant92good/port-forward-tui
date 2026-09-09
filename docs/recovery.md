# Network recovery and updates

[Back to the README](../README.md)

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
