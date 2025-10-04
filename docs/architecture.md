# Project architecture (October 2025)

## Stack

- Rust 2024 + PipeWire bindings for core audio plumbing
- Iced 0.13 desktop UI running on the `iced_wgpu` backend
- `async-channel` for cross-thread fan-out of registry snapshots and PCM frames

## Runtime building blocks

- **Virtual sink** (`audio::pw_virtual_sink`) – Owns the `openmeters.sink` PipeWire stream on a dedicated thread, negotiates `F32LE` stereo at 48 kHz, and drops captured frames into a process-wide `RingBuffer<Vec<f32>>`.
- **Meter tap** (`audio::meter_tap`) – Spawns a lightweight worker that drains the capture ring buffer and forwards frames over an async channel so UI visualisations can read audio without touching PipeWire threads.
- **Registry observer** (`audio::pw_registry`) – Mirrors PipeWire nodes, devices, and metadata defaults into cheap-to-clone `RegistrySnapshot` structs; exposes point-in-time snapshots plus a subscription stream.
- **Routing helper** (`audio::pw_router`) – Issues `target.*` metadata overrides so application streams follow either the virtual sink or the last-known hardware sink according to UI preferences.
- **Loopback controller** (`audio::pw_loopback`) – Keeps passive PipeWire links from the virtual sink’s monitor ports to the current hardware sink so users keep hearing routed audio locally.
- **Ring buffer** (`audio::ring_buffer`) – Generic fixed-capacity FIFO used by the virtual sink and meter tap to move `Vec<f32>` frames between threads.
- **Utilities** (`util::audio`, `util::pipewire`) – Helper APIs for sample-width conversion, metadata parsing, and PipeWire graph shaping that are shared across services.
- **Executable entrypoint** (`main.rs`) – Boots the registry observer, virtual sink, loopback, and meter tap; drives a `RoutingManager` that reacts to snapshots and launches the UI with routing and audio streams wired in.
- **UI layers** (`ui::*`) – An Iced application that lists routable clients, toggles overrides, and renders a modular LUFS meter (`ui::visualization::lufs_meter`) via a custom wgpu pipeline (`ui::render::lufs_meter`).

## Data flow snapshot

```text
applications ─┐
              ├─(metadata overrides via router)→ PipeWire → openmeters.sink → RingBuffer<Vec<f32>>
system audio ─┘                                          │
                                                         ├─ loopback → default hardware sink
                                                         └─ meter_tap → async channel → UI LufsProcessor → wgpu LUFS meter
```

The backend keeps playback audible while surfacing captured PCM to the UI, where a rolling RMS/peak processor feeds the wgpu LUFS widget.

## Operational notes

- Long-lived subsystems (`pw_registry`, `pw_virtual_sink`, `pw_loopback`, `meter_tap`) are protected by `OnceLock` singletons; shutdown remains best-effort.
- UI subscriptions batch registry snapshots and audio frames, keeping the rendering thread decoupled from PipeWire timing.
- The LUFS meter is intentionally modular: `ui::visualization` owns DSP/state, while `ui::render` houses the reusable wgpu primitive and shader.
