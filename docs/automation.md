# Commands for coding agents and scripts

[README](../README.md) · [Repository instructions](../AGENTS.md)

The compiled beta package uses **`ports-beta`**, with separate installation and
data ownership. Windows and Linux are supported targets; macOS remains beta.
This checkout is the 0.10.0-beta.1 source candidate; see the
[installation status](../README.md#install) before expecting a download.
`ports-beta` below is shorthand for the full installed path in the README:
beta deliberately adds no PATH entry. For a source build, substitute its exact
`target/debug/ports` path (`.exe` on Windows). Start with read-only inspection:

```sh
ports-beta doctor --json
ports-beta machines list --json
ports-beta list --json
```

Then choose explicit IDs for changes:

```sh
ports-beta machines add workbox --json
ports-beta save --machine MACHINE_ID --remote 8000 --name API --json
ports-beta save --machine MACHINE_ID --socks --local 1080 --name "Dev proxy" --json
ports-beta start FAVORITE_ID --machine MACHINE_ID --json
ports-beta stop FAVORITE_ID --machine MACHINE_ID --json
ports-beta auto-open FAVORITE_ID --machine MACHINE_ID --on --json
ports-beta auto-open FAVORITE_ID --machine MACHINE_ID --off --json
ports-beta delete FAVORITE_ID --machine MACHINE_ID --yes --json
```

Use the machine ID returned by `machines add` and the favorite ID returned by
`save`. Add `--local 18000` to use a different local port. With several machines,
writes require `--machine`; they never guess from terminal focus or a row number.
`save` requires exactly one mode: `--remote PORT` for a fixed forward, or
`--socks` for a proxy. A proxy's `--local` defaults to 1080; a fixed forward's
defaults to its remote port. Proxy destinations belong to client requests, so
`--socks --remote PORT` is rejected. Mapping reuse compares both kind and ports:
a proxy and a fixed forward are different favorites even if their local port
matches. They cannot listen on that same port simultaneously.

`save` saves without enabling a stopped favorite. It may start the local
controller to serialize edits. Reusing a mapping retains its ID; renaming an
active favorite may restart its SSH process. `start` waits up to five seconds
for a listener. Use `--wait 0` to return immediately, or up to 30 seconds.

`auto-open` changes only that favorite's **Open automatically** preference.
Exactly one of `--on` or `--off` is required. It does not start a controller or
change an existing connection. `list --json` includes `open_automatically`
on every favorite (default `false`). Invalid preference metadata is reported in
`automatic_opening_warning`; it is not silently reset.

Only opening a genuinely new TUI applies opted-in favorites, once across the
machines displayed at launch. Refreshes, machine selection within an existing
view, CLI commands and focusing an already-open view never apply them. Stop
leaves the preference enabled for the next new view. The TUI-only QUEUED label
is pending launch work, not a new controller/JSON state.

The TUI's **T** title preview and **U** display choice have no CLI equivalent.
They never change the saved `name` returned in JSON. Agents should use explicit
saved names; opening or listing Ports does not probe web pages.
SOCKS rows never receive the HTTP title check or the browser-opening action.
The TUI instead explains how to configure a client. See [SOCKS examples](socks.md).

## Results and exit codes

Beta JSON includes `schema_version: 2`, `channel: "beta"`, `version` and `ok`.
For example, a proxy item in `list` includes the following fields (IDs and state
are illustrative):

```json
{
  "id": "FAVORITE_ID",
  "name": "Dev proxy",
  "kind": "socks",
  "local_port": 1080,
  "url": "socks5h://127.0.0.1:1080",
  "state": "UNKNOWN",
  "open_automatically": false
}
```

Proxy items omit `remote_port`. Fixed-forward list items have `kind: "local"`,
an integer `remote_port` and their HTTP address. The endpoint string describes
how to use the listener; it is not a successful request or remote-health check.
Exit codes are **0** for successful
operations, **1** for an operational failure and **2** for invalid arguments.
A successful request does not necessarily mean a connection is ready: inspect
its returned state.

| State | Meaning |
| --- | --- |
| ON | SSH owns the local listening port; no destination health is implied |
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

`--data-dir PATH` selects beta-owned data. The default is `PortForwardTUI-Beta`
under the platform's local data directory. Beta rejects the stable data tree,
unmarked existing favorites, linked data paths and incompatible controller
endpoints before starting or editing. Do not add an ownership marker by hand.

To copy existing stable favorites, use a **new, nonexistent** destination:

```sh
ports-beta --data-dir NEW_BETA_DIRECTORY import-stable --from STABLE_DIRECTORY --json
```

This validates and copies saved machine/settings/forward metadata without
changing the source. It does not copy live controllers, credentials, or enabled
automatic-opening choices, and starts no connections. The beta then owns its
copy. It does not continuously synchronize back to the stable installation.
Beta uses controller protocol **2** and advertises `socks_proxy`; it cannot
share protocol-1 controllers with stable or legacy Python views. Local-only
saved files retain version 1 compatibility; once proxies are saved, their
file uses version 2 even if all proxies are later deleted. The JSON command
envelope remains schema version 2 in either case.

`machines discover --json`
previews static SSH aliases; `machines import --select workbox --json` imports
only the selected alias. Use `--config PATH` with either command for another
SSH configuration. See [machine import](machines.md).

`ports-beta machines pick --json` shows the interactive picker on stderr and emits
one JSON result on stdout after the terminal is restored. The result's
`machine` contains ID, name, target, SSH port/config and directory; cancellation
returns `machine: null`. `--force-picker` always prompts;
`--no-window-context` ignores Windows Terminal's saved view context.

After an update, close old TUI views and run
`ports-beta restart-manager --machine MACHINE_ID --json`. This briefly interrupts
that server, starts the new controller and restores only its previously
ON/CONNECTING/RETRYING requests. The result includes `restored_ids`.
OFF and ERROR favorites remain stopped. See [recovery](recovery.md).

An example instruction for your agent:

> Read AGENTS.md and run ports-beta doctor --json. Use my existing SSH alias workbox,
> save remote port 8000 as API, then start it. Report the observed state and local
> browser address. Preserve all other favorites and running connections.
