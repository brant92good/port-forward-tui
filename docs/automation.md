# Commands for coding agents and scripts

Run these commands from the installed app folder in Windows PowerShell. Terminal Workspace and a private parent expose the same `ports.ps1` commands.

[Back to the README](../README.md) · [Repository instructions](../AGENTS.md)


The same connections can be managed without opening the screen:

```powershell
.\doctor.ps1 --json
.\ports.ps1 machines add workbox --json
.\ports.ps1 list --machine workbox --json
.\ports.ps1 save --machine workbox --remote 8000 --name 'My web app' --json
# Copy the returned id into the next command:
.\ports.ps1 start FAVORITE_ID --machine workbox --json
.\ports.ps1 stop FAVORITE_ID --machine workbox --json
.\ports.ps1 delete FAVORITE_ID --machine workbox --yes --json
```

`save` keeps a stopped connection stopped. It can start the local background
manager, which serializes edits from all screens and agents. Saving an existing
mapping reuses its ID; a name change to an active favorite may restart it.
`--local 18000` chooses a different port here. `start` waits up to five seconds
for a listener; inspect the returned state, since a slow connection can still
be `CONNECTING` or `RETRYING`. `RETRYING` means the request remains enabled and
the controller will make another network attempt; it does not mean a listener
is ready. `--wait 0` returns immediately. `stop` cancels pending retries too.
`stop-all` affects every
connection for the selected machine. With multiple saved machines, writes require
`--machine ID`; `list` without it returns connections labeled by machine.
[Machine commands and import](machines.md#commands-for-an-agent) cover first use.

Results contain `schema_version: 1` and `ok`. Exit codes are **0** for success,
**1** for a failed check/operation, and **2** for invalid arguments. `list` and
`doctor` do not create favorites or start the manager. `UNKNOWN` means live
status was not available; it does not mean the tunnel has stopped.
Use `--data-dir 'C:\path\to\separate-data'` for a separate set of connections.
JSON can contain private host and favorite names; review it before sharing.

After an app update, `restart-manager --machine MACHINE_ID --json` loads the
new controller and returns `restored_ids`. It briefly interrupts that server's
connections and restores only ON/connecting/retrying requests. OFF and ERROR
favorites stay stopped. Close old TUI views first and reopen them afterward;
they otherwise keep running their earlier code. The command does not reboot
Windows or touch another server's controller.

The TUI displays server groups together; CLI favorite IDs remain unchanged
within each machine. Use the pair `machine_id` and `id` from `list --json` for
writes. Never infer a machine from a row number or an identical name on another
server. A TUI's S key stops all listed servers; the CLI's `stop-all --machine`
deliberately retains its explicit single-machine scope.

An example request for your agent:

> Read AGENTS.md and run doctor.ps1 --json. Explain any missing prerequisites.
> My existing SSH name is YOUR_SSH_NAME. Install with -NonInteractive, add that machine, then save
> remote port 8000 as My web app using its machine id. Start that favorite and report its state and
> browser address. Preserve my other favorites and connections.

Replace `YOUR_SSH_NAME` before using that request. Noninteractive installation does not require a host or open a login prompt.
