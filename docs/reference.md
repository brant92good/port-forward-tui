# Architecture and development

Ports is a Rust TUI and CLI around the system OpenSSH client. It keeps saved
metadata, tunnel intent, process ownership and screen state in separate modules.

| Component | Responsibility |
| --- | --- |
| `src/store.rs`, `machines.rs` | Validated favorites, stable machine IDs, atomic writes, static SSH alias import |
| `src/forwarding.rs` | ON/OFF intent, retry timing, cancellation and permanent error classification |
| `src/auto_open.rs` | Opt-in new-view preferences and frozen automatic-start requests |
| `src/forward_form.rs` | Saved-forward editor, checked preference updates and partial-save reporting |
| `src/connection_list.rs` | Name-first rows under machine headings, with selected-row scrolling |
| `src/service_name.rs` | Explicit, bounded local HTTP title preview and view-only labels |
| `src/process/` | Owned SSH process groups/jobs and OS listener ownership |
| `src/background.rs` | Per-machine detached controller, protocol-1 IPC and serialized edits |
| `src/cli.rs` | Commands and versioned JSON results |
| `src/ui.rs`, `picker.rs`, `screen.rs` | Combined view, machine picker and keyboard forms |
| `src/views.rs`, `native/` | Optional Windows Terminal identity/focus integration |

The controller launches OpenSSH with loopback-only local forwarding, strict
host-key checking, noninteractive authentication and keepalives. It determines
ON from the owning process's listener, without probing the remote service.
Windows uses owned jobs; Unix uses separate process groups. Stopping affects
owned processes, including ProxyCommand descendants.

Views talk to an authenticated loopback controller through bounded JSON
messages. A per-machine lock selects the controller; two views attach to it.
Favorites are written atomically, and edits/deletes carry their previous value
to reject conflicting changes. Protocol 1 and saved schemas preserve the
earlier client's data contract during migration.

## Data

Default data folders follow the OS's local application-data location:

- Windows: `%LOCALAPPDATA%/PortForwardTUI`
- Linux: `$XDG_DATA_HOME/PortForwardTUI`, normally `~/.local/share/PortForwardTUI`
- macOS: `~/Library/Application Support/PortForwardTUI`

`--data-dir PATH` overrides that location. An existing root `forwards.json`
remains a machine. Additional machines have their own folders under `machines/`.
Custom SSH config paths are local filesystem references, passed to OpenSSH
with `-F`. Never publish endpoint tokens, keys or personal connection metadata.

Each machine's optional `forward-options.json` uses
`{"version":1,"open_automatically":["FAVORITE_ID"]}`. Missing means every
favorite is disabled. A separate short-lived lock protects atomic preference
merges; old controllers continue reading the unchanged `forwards.json` schema.
Deleted IDs are inert. Malformed or unsupported options disable automatic work
for that machine and display a warning, while manual controls remain available.

A new TUI captures opted-in favorites and their machine destination settings.
It processes one automatic request at a time, prioritizes manual actions and
rechecks for changed/deleted preferences, favorites and destinations before
dispatch. Starts use the existing requested-port conflict checks and idempotent
protocol-1 command. Q/Stop cancel this view's unsent work; at most the current bounded
request finishes before a queued stop. No refresh rebuilds this launch queue.

E edits connection fields and the automatic-opening checkbox together. The
sidecar remains separate from the controller-owned favorite file: the editor
checks the original snapshots before changes, then checks the newly saved
favorite before merging its option. A later failure reports which fields were
actually saved and retains the dialog; it does not claim a two-file transaction.
Checkbox-only changes never call the controller. F2 and CLI metadata edits remain
compatible with earlier controllers.

## Explicit web titles

Only T on a selected ON row starts a title check. The worker freezes the machine,
directory, destination and full favorite, validates current state, then requests
`http://127.0.0.1:LOCAL_PORT/`. It disables proxy discovery and redirects, accepts
only status 200 with uncompressed HTML, and bounds headers to 16 KiB, the body to
64 KiB and the HTTP request to 1.5 seconds. UTF-8/ASCII titles must contain 1–80
readable characters; control and directional override text is rejected. It does
not execute scripts or authenticate to the web app.

The result is the page's own label, not verified application identity. U adopts
it in this view only; Enter/E return to the saved name. Changed/stopped forwards
invalidate the preview. No favorite, controller protocol or JSON name changes.

## Windows integration

Windows bundles contain `ports.exe` plus compiled focus helpers. The helpers
use Windows' existing .NET Framework/UI Automation facilities; they do not need
Python or a compiler on the user's computer. Rust owns the TUI, CLI, controller
and process logic. Windows builds statically link the Rust C runtime dependency.

`--focus-existing` requests a matching existing view; ordinary launches create
another view. Native records use tab identity, process lifetime and recent
focus. F2 chooses same-window or all-window scope. This integration is specific
to Windows Terminal; the portable forwarding app does not require it.

## Build and check

Development requires Rust 1.88 or newer; release CI pins its toolchain. Use:

```sh
cargo test --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo run -- --data-dir ./example-data machines add workbox
cargo run -- --data-dir ./example-data
```

Tests use isolated data and owned processes. `native_pty` opens an OS
pseudo-terminal, not a desktop window. `native_process` compiles a test-only
SSH-shaped process with rustc to check listener/descendant ownership and recovery.
Release builds use static CRT on Windows and musl on Linux; macOS is beta.

`scripts/build_native.ps1` builds Windows helpers into an artifact folder.
`scripts/package_native.ps1` / `package_native.py` create binary bundles.
The Python packaging and compatibility utilities are development tools, not
installed application dependencies. Old Python source is retained for migration
tests and historical releases; production commands use the compiled binaries.

Generate documentation images with `cargo run --locked --features screenshots
--example capture`. The feature enables synthetic state for the actual widgets;
the capture does not open SSH or make HTTP requests. [Frame output measurements](rendering.md)
describe the buffered writer separately from physical terminal painting.

Bundles include the app license and [third-party notices](licenses/README.md).
`python scripts/collect_licenses.py --check` verifies the five-target locked
dependency inventory before packaging. The installers validate those documents
alongside the binaries; only explicit releases through 0.8.1 accept the original
bundle shape without notices.
