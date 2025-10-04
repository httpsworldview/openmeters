# Project architecture (October 2025)

## Stack

Rust + PipeWire today, with an `iced` desktop UI planned but not yet wired up.

## Runtime building blocks

- **Virtual sink** (`audio::pw_virtual_sink`) – Owns the `openmeters.sink` PipeWire stream on a dedicated thread, negotiates `F32LE` stereo (48 kHz by default), and writes captured buffers into a process-wide `RingBuffer<Vec<f32>>` exposed through `capture_buffer_handle()`.
- **Registry observer** (`audio::pw_registry`) – Runs a background thread that mirrors PipeWire nodes, devices, and default-target metadata into cheap-to-clone `RegistrySnapshot` structs and delivers them to subscribers via `RegistryUpdates`.
- **Routing helper** (`audio::pw_router`) – Maintains a secondary PipeWire connection, binds a metadata object, and issues `target.*` overrides so application streams follow the chosen sink announced by the registry monitor.
- **Loopback controller** (`audio::pw_loopback`) – Listens to live graph events, tracks the OpenMeters monitor ports, and sustains passive links from those outputs to the current default hardware sink so playback continues locally.
- **Ring buffer** (`audio::ring_buffer`) – Generic, fixed-capacity FIFO with iterators and batch helpers, used to hand off captured `Vec<f32>` frames to future DSP/telemetry consumers.
- **Utilities** (`util::pipewire`, `util::audio`) – Shared helpers for dictionary cloning, graph/port shaping, metadata parsing, routing payloads, and sample-format conversion.
- **Executable entrypoint** (`main.rs`) – Boots the registry observer, virtual sink, and loopback; spins up a `RoutingManager` that reacts to registry snapshots, logs lifecycle events, and parks the process after initialisation.

## Data flow snapshot

```text
applications ─┐
              ├─(metadata hints via router)→ PipeWire → openmeters.sink → RingBuffer<Vec<f32>>
system audio ─┘                                          │
                                                         └─ passive links → default hardware sink
```

Incoming audio is captured for analysis while the router steers client streams into the virtual sink and the loopback keeps those samples audible on the user’s active device.

## Operational notes

- Long-lived subsystems (`pw_registry`, `pw_virtual_sink`, `pw_loopback`) are spawned exactly once via `OnceLock`; there is no coordinated shutdown yet.
- Metadata helpers (`util::pipewire::metadata`) power both the registry defaults cache and the router’s target payloads.
- UI scaffolding in `src/ui.rs` is empty; when the Iced frontend materialises it will consume the shared capture buffer and registry snapshots.
