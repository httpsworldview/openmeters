# Project architecture (October 2025)

## Stack

Rust + PipeWire today, with Qt6/OpenGL planned for the UI layer.

## Runtime building blocks

- **Virtual sink** (`audio::pw_virtual_sink`) – Registers the `openmeters.sink` node, negotiates `F32LE` stereo at 48 kHz, captures incoming buffers into a shared `RingBuffer<Vec<f32>>`, and keeps the PipeWire stream serviced on a dedicated thread.
- **Registry observer** (`audio::pw_registry`) – Mirrors PipeWire node/device metadata into `RegistrySnapshot` structures, tracks default sink/source updates via shared metadata utilities, and broadcasts snapshots to subscribers.
- **Loopback controller** (`audio::pw_loopback`) – Watches the registry view, discovers the OpenMeters monitor ports, and maintains passive links from those outputs to the current default hardware sink so audio continues to the user’s devices.
- **Ring buffer** (`audio::ring_buffer`) – Generic, fixed-capacity FIFO used by the virtual sink and ready for future DSP consumers.
- **Utilities** (`util::pipewire`, `util::audio`) – Common bindings for PipeWire dictionary parsing, graph metadata, and audio-format helpers shared across the audio stack.
- **Executable entrypoint** (`main.rs`) – Boots the registry observer, virtual sink, and loopback, logs lifecycle events, spawns lightweight telemetry threads, and parks the process.

## Data flow snapshot

```text
app → PipeWire → openmeters.sink → (captured frames → RingBuffer)
                          │
                          └─ PipeWire links → default hardware sink
```

The virtual sink captures samples for analysis, while the loopback keeps playback routed to the listener’s chosen output without manual intervention.

## Operational notes

- Each subsystem starts exactly once via `OnceLock`; no shutdown path yet.
- Registry-derived helpers (default targets, graph metadata) live in `util::pipewire` and are consumed by both loopback and registry observers.
- Qt/OpenGL UI hooks are stubbed in `ui/` awaiting implementation; the current surface is entirely CLI/log driven.
