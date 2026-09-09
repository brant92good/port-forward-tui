# Architecture and development

Ports is a Rust TUI and CLI around the system OpenSSH client. It keeps saved
metadata, tunnel intent, process ownership and screen state in separate modules.

| Component | Responsibility |
| --- | --- |
| `src/store.rs`, `machines.rs` | Validated favorites, stable machine IDs, atomic writes, static SSH alias import |
| `src/forwarding.rs` | ON/OFF intent, retry timing, cancellation and permanent error classification |
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
cargo clippy --locked --all-targets -- -D warnings
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
