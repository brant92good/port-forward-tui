# Machines and SSH import

Install first; choose machines afterward. Ports uses OpenSSH and works
independently of Herdr or any particular terminal app.

## Add or import

Run `ports`. **A** adds an SSH alias such as `workbox` or a destination such as
`alex@server.example.com`. Give it an optional friendly name. Leave the SSH
login port blank to use your SSH configuration (normally 22); this is separate
from the app port you want to forward. An optional config path is passed to
OpenSSH with `-F`.

**I** previews aliases from `~/.ssh/config` (the home directory on each OS), or
another file you select. **Space** selects names, **A** selects all, and
**Enter** imports selected names—or just the highlighted name when none are
checked. Adding or importing does not connect to the server.

Import reads literal Host names and static Include files with depth/file limits.
Wildcard and negated patterns are excluded. Dynamic paths with percent tokens
are skipped; add those aliases manually. Imported names may be conditional;
OpenSSH evaluates their settings when you connect. The importer does not execute
Match commands or rewrite your configuration.

Your existing OpenSSH configuration supplies keys, jump hosts and ProxyCommand
settings. Keep an imported nondefault configuration at its recorded path.

## Several servers in one view

The main list includes every saved server, with its name in the SERVER column.
Move to a row to choose where quick entry and **A** add a forward. An empty
machine still has a selectable row. The header and add form show the selected
server.

**H** opens the machine picker. If you are typing in quick entry, press **Esc**
first. Existing forwards continue while you choose or add another machine.
Each open view keeps its own selection; favorites and live state are shared.
**S** asks before stopping all listed servers, including pending retries.
`--foreground` instead runs one machine and stops its forwards when that view
closes; machine switching is not offered in that mode.

Two servers can both have remote port 8000. Use local 8000 for one and local
18000 for the other. The UI rejects using another enabled connection's local
port, even while that connection is retrying. It does not stop another server's
forward to take the port.

Run `ports --machines` to always show the picker, or
`ports --machine MACHINE_ID` to select that server initially.
`ports --host workbox` adds or opens that destination.

## Windows Terminal integration

[Terminal Workspace](https://github.com/brant92good/terminal-workspace) pairs
remote tabs with Ports and adds return shortcuts. A Ports view records the
machine selected in its list. Return shortcuts use the invoking window's known
machine context, then choose that machine's most recently focused matching tab.
**F2** allows searching across windows or restricts it to the current window.
Without a known context, multiple saved machines prompt a picker.

These shortcuts belong to the Windows integration. The core Ports app runs in
other terminals too.

## Existing data and updates

The original `forwards.json` remains a machine. Extra machines live in
`machines/` under the same local data directory. Favorite IDs and saved ports
are retained. No repository or account is required.

Destinations are immutable: add a new machine to change its SSH alias, login
port or config path. Do not redirect a live controller by editing its data file.

Running views and controllers retain their loaded code. Close old views after
an update, then use `ports restart-manager --machine MACHINE_ID --json` to
replace that server's controller while restoring its enabled requests.
[Update behavior](recovery.md) · [CLI commands](automation.md).

