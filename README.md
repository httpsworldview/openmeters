# OpenMeters

![Preview](https://github.com/user-attachments/assets/94b2a531-c93b-41c3-9477-20cb3b9bc046)

OpenMeters is a fast and simple audio metering application for Linux,
built with Rust and PipeWire.

## Features

Checked items are implemented; unchecked items are planned. If you
have ideas for more features, please feel free to open an issue or
pull request!

### General

- [x] Bar mode
  - Places the application at the top or bottom of the screen,
    spanning the entire width.
  - (requires a Wayland compositor, unavailable on X11 as of now)
- [x] Per-application capture
- [x] Per-device capture
- [x] Pop-out windows for individual visuals
- [x] Adjustable background color & opacity
- [x] Ability to enable/disable window decorations

### Visuals

- [x] **loudness**
  - [x] LUFS (ITU-R BS.1770-5)
    - [x] Short-term
    - [x] Momentary
  - [x] RMS
    - [x] Fast
    - [x] Slow
  - [x] True Peak
- [x] **oscilloscope**
  - [x] Channel selection (L, R, L+R)
  - [x] Stable mode - follows X cycles of the fundamental.
  - [x] Free-run mode - scrolls continuously through time, not
        triggered.
- [x] **spectrogram**
  - [x] Reassignment and synchrosqueezing for sharper frequency and
        time resolution.
  - [x] Note & frequency tool tips
  - [x] Piano roll overlay
  - [x] Ability to zoom & pan vertically
  - [x] Mel, log, and linear scales
  - [x] Adjustable colormap
- [x] **spectrum analyzer**
  - [x] Peak frequency label
  - [x] Averaging modes
    - [x] Exponential
    - [x] Peak hold
    - [x] None
  - [x] Mel, log, and linear scales
  - [x] Bar mode
  - [x] Adjustable color map
- [x] **stereometer** (X/Y vector scope, M/S goniometer)
  - [x] Correlation meter
    - [x] Single or multi-band
    - [x] Adjustable time window
  - [x] Two visual modes:
    - [x] Lissajous (draws lines between samples)
    - [x] Dot cloud (plots samples as points)
  - [x] Ability to flip L/R channels (for M/S monitoring)
  - [x] Adjustable scale (linear/exponential)
  - [x] Adjustable rotation
  - [x] Grid overlay
- [x] **waveform**
  - [x] Channel selection (L, R, L+R)
  - [x] Adjustable scroll speed
  - [x] Adjustable color map

## Notes on Performance

OpenMeters is designed to be efficient and lightweight, but real-world
performance will vary greatly depending on your system and settings.
Settings within the GUI are intentionally ***very*** flexible and can
have a ***significant*** impact on performance.  General performance
notes:

- ~500MB RAM usage with all visuals active and default settings.
- CPU usage between 0.5-5% on a modern processor.
- GPU usage between 1-20% on a somewhat modern chip.

## Installation

### On Arch Linux (and Arch-based distributions)

Install the `openmeters-git` package via the AUR.

```bash
yay -S openmeters-git
```

### Building from source (Other distributions)

1. **You'll need a graphical Linux system with PipeWire installed and
   running.**
2. Ensure you have a working Rust toolchain. The recommended way is
   via [rustup](https://rustup.rs/).
3. Clone the repository:

   ```bash
   git clone https://github.com/httpsworldview/openmeters/
   cd openmeters
   ```

4. Build and run the application in release mode:

   ```bash
   cargo build -r
   ./target/release/openmeters
   ```

   or run it directly with Cargo:

   ```bash
   cargo run -r
   ```

## Usage & key binds

### Global

- `ctrl+shift+h`: Show/hide global configuration drawer
- `p`: Pause/resume rendering.
- `q` twice: Quit the application.
- `ctrl+space`: Move a hovered visual to a new window, or back to the
  main window.

### Spectrogram

- `ctrl+scroll up/down`: Zoom vertically
- `middle click+drag`: Pan vertically

### Configuration

- Configurations are saved to `~/.config/openmeters/settings.json`.
- Invalid JSON will be ignored and default settings will be used
  instead. Your configuration file will not be overwritten unless you
  change settings in the GUI.
- The internal structure of this file will likely change often during
  development, so be aware that your settings **may be reset
  inexplicably after updates**. As this project grows, I will try to
  maintain backwards compatibility as much as possible, but no
  guarantees are made. The public API for settings is mostly stable as
  of now, so breaking changes should be infrequent.
- If you encounter a bug that causes OpenMeters to misbehave, the
  application settings can be reset by deleting this file. Please
  consider reporting any such bugs you encounter.

## Credits

Thank *you* for checking out my shitty passion project. If you think
OpenMeters is useful, please consider starring the repository and
sharing it with others. I appreciate any and all criticism and
feedback, so feel free to reach out to me.

### Inspiration

- **EasyEffects** (<https://github.com/wwmm/easyeffects>) for being a
  great source of inspiration and for their excellent work in audio
  processing. Reading through their codebase taught me a lot about
  PipeWire.
- **MiniMeters** (<https://minimeters.app/>) for inspiring this entire
  project and for doing it better than I ever could. If you can,
  please support their work!
- **Ardura's Scrolloscope** (<https://github.com/ardura/Scrollscope>)
- **Tim Strasser's Oszilloskop**
  (<https://github.com/timstr/oszilloskop>)
- **Audacity** (<https://www.audacityteam.org/>)

### Libraries used

- **iced_layershell** and related crates
  (<https://github.com/waycrate/exwlshelleventloop>)
  - Special thanks to Decodetalkers for reviewing and merging my
    patches.
- **Iced** (<https://github.com/iced-rs/iced>)
- **RustFFT** (<https://github.com/ejmahler/RustFFT>)
- **RealFFT** (<https://github.com/HEnquist/realfft>)
- **wgpu** (<https://github.com/gfx-rs/wgpu>)
