# Frame output measurement

The 0.9 candidate buffers terminal output at completed-draw boundaries. The old
writer sent thousands of tiny fragments to Windows' pseudo-terminal. In a
controlled comparison, buffering delivered the same frame bytes much sooner.
These are local source measurements, not published-release or desktop claims.

| Viewport | Writer | Underlying writes | Frame bytes | Process start to required content |
| --- | --- | ---: | ---: | ---: |
| 120 × 32 | Previous unbuffered writer | 3,967 | 5,489 | 210.58 ms |
| 120 × 32 | Buffered writer | 2 | 5,489 | 34.03 ms |
| 220 × 65 | Previous unbuffered writer | 14,848 | 17,996 | 777.90 ms |
| 220 × 65 | Buffered writer | 5 | 17,996 | 34.68 ms |

Measured September 11, 2026 (Taipei), in an owned Windows ConPTY with 40 disabled
example forwards. Each row is the mean of six runs after one warmup, alternating
the two writers. Both variants used the same executable, rendering callback,
data and viewport. Emitted-byte hashes matched in every paired run. The
[raw samples](benchmarks/frame-writers-windows.json) include warmups, byte counts
and the exact probe hash. The source base was `35fb915` with the buffering draft;
this is not a clean-release comparison.

Completion required recognizable frame content and footer text. This measures
**output-byte arrival, not physical screen paint, hotkey-to-window latency or
network throughput**. Title and required content sometimes arrived in one read
chunk; that is not zero rendering work. The internal `draw_ms` also includes
per-write instrumentation, so the table uses externally observed arrival times.
Terminal, scheduler and viewport differences can change the result.

`FrameWriter` buffers up to 64 KiB. Large frames can still split; arbitrary-size
updates are not atomic. On teardown the writer finishes while the alternate
screen is active, discards remaining bytes after an output failure, and does
not replay a partial frame into the shell when dropped. Output regressions
verify byte preservation and that failure boundary. An actual ConPTY lifecycle
test covers the main view, Help, Settings, Edit, resizing and quit.

From a developer checkout:

```sh
cargo test --locked --lib screen::output_tests
cargo test --locked --test native_render
cargo run --release --locked --example render_probe -- --writers
```

The probe uses temporary example data and owns its subprocesses. It does not
read a personal catalog or open SSH. The older [CLI startup benchmark](performance.md)
measures a different operation and should not be added to these timings.
