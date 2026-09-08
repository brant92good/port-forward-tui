# Commands for coding agents and scripts

Run these commands from the installed app folder in Windows PowerShell. Terminal Workspace and a private parent expose the same `ports.ps1` commands.

[Back to the README](../README.md) · [Repository instructions](../AGENTS.md)


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
