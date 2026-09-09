# Network recovery and updates

[Back to the README](../README.md)

## When a laptop loses its connection

An enabled forward moves to **RETRYING** after SSH detects a broken connection.
The controller waits 2, 4, 8, 16, then at most 30 seconds between failed attempts.
When the route becomes reachable, it recreates the same forward—even with the
view closed. Detection and login also take time.

**Enter stops retries**, **R retries now**, and **S stops all listed servers**.
OFF favorites stay OFF. A connection must stay healthy for 30 seconds before
its retry delay resets. Authentication failures, changed or untrusted host
keys, and occupied local ports show ERROR. Fix the cause before restarting.

A restored forward cannot restore an interrupted application connection. Refresh
your browser or reconnect your database client if needed. Reboot, sign-out and
a terminated controller end running connections; they do not automatically
restore active state.

## Updating

Run the [installer](../README.md#install) again to download the compiled update.
Your saved connections live outside the installed binaries and remain in place.

Close old TUI views, open a new terminal and run:

```sh
ports machines list --json
ports restart-manager --machine MACHINE_ID --json
```

This briefly interrupts that server's forwards and restores only requests that
were ON, CONNECTING or RETRYING. OFF and ERROR favorites stay stopped.
Other servers are unaffected. Repeat for each controller you want to update.
Parent setups should follow the leaf versions recorded by their own installation.

## Troubleshooting

- **Login or host-key error:** run `ssh YOUR_ALIAS` in a shell and resolve the
  actual SSH error. Ports uses noninteractive background SSH with strict host-key
  checking.
- **Encrypted key needs a passphrase:** load it through your OpenSSH agent before
  starting a background forward.
- **Local port in use:** stop its existing connection or choose another local
  port, such as `18000:8000`.
- **ON but the browser fails:** ON verifies the SSH listener, not the remote app.
  Check that the app is serving the chosen port on the server.
- **UNKNOWN:** the controller could not be reached. Existing forwards may still
  be running; inspect `ports doctor --json` before restarting anything.
- **Controller fails to detach:** a restrictive host application may forbid
  background child processes. Try a normal terminal, or use `--foreground` and
  keep that view open. Report the terminal and OS in an issue.

