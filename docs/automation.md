# Commands for coding agents and scripts

[README](../README.md) · [Repository instructions](../AGENTS.md)

The compiled `ports` command works in PowerShell, Linux and macOS terminals.
Start with read-only inspection:

```sh
ports doctor --json
ports machines list --json
ports list --json
```

Then choose explicit IDs for changes:

```sh
ports machines add workbox --json
ports save --machine MACHINE_ID --remote 8000 --name API --json
ports start FAVORITE_ID --machine MACHINE_ID --json
ports stop FAVORITE_ID --machine MACHINE_ID --json
ports delete FAVORITE_ID --machine MACHINE_ID --yes --json
```

Use the machine ID returned by `machines add` and the favorite ID returned by
`save`. Add `--local 18000` to use a different local port. With several machines,
writes require `--machine`; they never guess from terminal focus or a row number.

`save` saves without enabling a stopped favorite. It may start the local
controller to serialize edits. Reusing a mapping retains its ID; renaming an
active favorite may restart its SSH process. `start` waits up to five seconds
for a listener. Use `--wait 0` to return immediately, or up to 30 seconds.

## Results and exit codes

JSON includes `schema_version: 1` and `ok`. Exit codes are **0** for successful
operations, **1** for an operational failure and **2** for invalid arguments.
A successful request does not necessarily mean a connection is ready: inspect
its returned state.

| State | Meaning |
| --- | --- |
| ON | SSH owns the local listening port |
| CONNECTING | SSH is starting |
| RETRYING | Still enabled; another network attempt is scheduled |
| OFF | Stopped, with no pending retry |
| ERROR | Needs attention before restarting |
| UNKNOWN | Live state could not be observed |

`list` and `doctor` do not create files or start controllers. Treat UNKNOWN as
unknown, not permission to take a port. Use the pair of machine ID and favorite
ID when operating on the combined list. JSON may include private host names.

`stop` cancels retries. `stop-all --machine MACHINE_ID` stops one server.
The TUI's **S** key instead stops every server shown in its combined list.
UI start actions check other servers' enabled ports, including RETRYING rows.
Separate CLI requests still rely on their controller and the operating system's
bind checks; there is no atomic port reservation across independent controllers.

## Configuration and integration

`--data-dir PATH` selects an isolated data directory. `machines discover --json`
previews static SSH aliases; `machines import --select workbox --json` imports
only the selected alias. Use `--config PATH` with either command for another
SSH configuration. See [machine import](machines.md).

`ports machines pick --json` shows the interactive picker on stderr and emits
one JSON result on stdout after the terminal is restored. The result's
`machine` contains ID, name, target, SSH port/config and directory; cancellation
returns `machine: null`. `--force-picker` always prompts;
`--no-window-context` ignores Windows Terminal's saved view context.

After an update, close old TUI views and run
`ports restart-manager --machine MACHINE_ID --json`. This briefly interrupts
that server, starts the new controller and restores only its previously
ON/CONNECTING/RETRYING requests. The result includes `restored_ids`.
OFF and ERROR favorites remain stopped. See [recovery](recovery.md).

An example instruction for your agent:

> Read AGENTS.md and run ports doctor --json. Use my existing SSH alias workbox,
> save remote port 8000 as API, then start it. Report the observed state and local
> browser address. Preserve all other favorites and running connections.

