# Real-workload allocation traces (plan §7.3)

`.ratrace` files recorded from our own products by a shim allocator:

- spacedb ingest + query soak (CRDT merge churn)
- rusty_h264 / remade_ffmpeg_rs encode+decode (frame-buffer lifecycle)
- FFAI inference session (tensor arena pattern)
- mata-master dioxus desktop startup + interaction

Format: `crates/rusty_alloc_bench/src/trace.rs` (v0, 32-byte records, block *ids*
never addresses). Files are named `<workload>-<content-hash8>.ratrace` and are
immutable once referenced by a LEDGER entry — re-record under a new name.

Traces are large; only hashes + provenance notes are committed here. The files
live on the bench box / artifact storage (pointer per trace in `index.md`).

Recorder shim: lands with M2. It shims over the *system* allocator (recording
needs nothing from rusty_alloc) — it is scheduled at M2 simply because its first
consumer is the M2 G2 gate.
