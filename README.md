# OpenMeters

![Preview](https://github.com/user-attachments/assets/94b2a531-c93b-41c3-9477-20cb3b9bc046)

OpenMeters is a fast and simple audio metering application for Linux, built with Rust and PipeWire. It's in active development, so expect bugs and broken configurations. I'm always open to feedback and bug reports, so feel free to open issues.

## features

Checked features are implemented, unchecked features are planned. If you have ideas for more features, please feel free to open an issue/pull request!

### general

- [x] Per-application capture
- [x] Per-device capture
- [x] Pop-out windows for individual visuals
- [x] Adjustable background color & opacity
- [x] Ability to enable/disable window decorations

### visuals

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
  - [x] Stable mode - Follows X cycles of the fundamental.
  - [x] Free-run mode - Scrolls continuously through time, not triggered.
- [x] **spectrogram**
  - [x] Reassignment and synchrosqueezing (sharper frequency resolution)
  - [x] Note & frequency tooltips
  - [x] Piano roll overlay
  - [x] Ability to zoom & pan vertically
  - [x] mel, log, and linear scales
  - [x] Adjustable colormap
- [x] **spectrum analyzer**
  - [x] peak frequency label
  - [x] averaging modes
    - [x] exponential
    - [x] peak hold
    - [x] none
  - [x] mel, log, and linear scales
  - [x] bar mode
  - [x] Adjustable colormap
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
- [x] **waveform**
  - [x] Channel selection (L, R, L+R)
  - [x] adjustable scroll speed
  - [x] Adjustable colormap

## build and run

1. **You'll need a graphical Linux system with PipeWire installed and running.**
2. Ensure you have a working Rust toolchain. The recommended way is via [rustup](https://rustup.rs/).
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

## usage & keybinds

### global

- `ctrl+shift+h`: Show/hide global configuration drawer
- `p`: Pause/resume rendering.
- `q` twice: Quit the application.
- `ctrl+space`: Move a hovered visual to a new window, or back to the main window.

### spectrogram

- `ctrl+scroll up/down`: Zoom vertically
- `middle click+drag`: Pan vertically

### configuration

- Configurations are saved to `~/.config/openmeters/settings.json`. If you want to use settings/values not listed in the GUI, you can edit this file directly, however absurd/invalid values may cause crashes (or be ignored).
- Configurations are validated on startup. Invalid JSON will be ignored and default settings will be used instead. Your configuration file will not be overwritten unless you change settings in the GUI.
- The internal structure of this file will likely change often during development, so be aware that your settings **may be reset inexplicably after updates**. As this project grows, I will try to maintain backwards compatibility as much as possible, but no guarantees are made. The public API for settings is mostly stable as of now, so breaking changes should be infrequent.
- If you encounter a bug that causes OpenMeters to misbehave, the application settings can be reset by deleting this file.

## credits

Thank *you* for checking out my shitty passion project. If you think OpenMeters is useful, please consider starring the repository and sharing it with others. I appreciate any and all criticism and feedback, so feel free to open issues or reach out to me.

### inspiration

- **EasyEffects** (<https://github.com/wwmm/easyeffects>) for being a great source of inspiration and for their excellent work in audio processing. Reading through their codebase taught me a lot about PipeWire.
- **MiniMeters** (<https://minimeters.app/>) for inspiring this entire project and for doing it better than I ever could. If you can, please support their work!
- **Ardura's Scrolloscope** (<https://github.com/ardura/Scrollscope>)
- **Tim Strasser's Oszilloskop** (<https://github.com/timstr/oszilloskop>)
- **Audacity** (<https://www.audacityteam.org/>)

### libraries used

- **Iced** (<https://github.com/iced-rs/iced>)
- **RustFFT** (<https://github.com/ejmahler/RustFFT>)
- **RealFFT** (<https://github.com/HEnquist/realfft>)
- **wgpu** (<https://github.com/gfx-rs/wgpu>)
