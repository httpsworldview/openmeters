# OpenMeters

https://github.com/user-attachments/assets/52d0202e-f6e7-47aa-9952-e3a0be975f42

OpenMeters is an audio metering application for Linux. It can monitor
individual PipeWire applications, capture from devices, and display
the signal through a set of practical meters: loudness, oscilloscope,
spectrogram, spectrum analyzer, stereometer, and waveform.

My goal is to provide a free, open source desktop meter that is clear
at a glance, rigorous and correct about what it computes, and pleasant
enough to want to keep running.

## Quick links

- [Features](#features)
- [Installation](#installation)
  - [Arch Linux](#arch-linux)
  - [Debian](#debian)
  - [Fedora](#fedora)
  - [Build from source](#building-from-source)
- [Usage and key bindings](#usage-and-key-bindings)
- [Configuration and theming](#configuration)
- [Contributing](#contributing)
- [Credits](#credits)
- [License](#license)

## Features

If you have ideas for the next thing OpenMeters should do, please feel
free to open an issue or pull request.

### General

- PipeWire audio capture
  - Per-application capture through a virtual sink.
  - Per-device capture for monitoring selected devices or the system
    default.
  - Application routing is restored on clean shutdown.
- Windowing
  - Normal desktop windows on X11 or Wayland.
  - Wayland bar mode, placing OpenMeters at the top or bottom of the
    selected monitor.
  - Pop-out windows for individual visuals.
  - Optional window decorations.
- Appearance and persistence
  - Adjustable background color and opacity.
  - Editable settings stored as JSON.
  - User themes stored as separate JSON files.

### Visuals

- **Loudness**
  - LUFS short-term and momentary metering according to ITU-R
    BS.1770-5.
  - Fast and slow per-channel RMS meters.
  - Per-channel true peak metering.
- **Oscilloscope**
  - Left, right, and summed-channel views.
  - Stable triggering based on pitch estimation and phase correlation.
  - Zero-crossing triggering for traditional scope behavior.
  - Selectable cycle count in stable mode.
- **Spectrogram**
  - Classic STFT mode.
  - Time-frequency reassignment.
  - Note and frequency tooltips.
  - Piano-roll overlay.
  - Vertical zoom and pan.
  - ERB, logarithmic, and linear frequency scales.
  - Adjustable color map.
- **Spectrum analyzer**
  - Windowed real FFT analysis.
  - IEC 61672-1 A-weighting.
  - Peak frequency label.
  - Exponential averaging, peak hold, or no averaging.
  - ERB, logarithmic, and linear frequency scales.
  - Bar mode and adjustable color map.
- **Stereometer**
  - X/Y vector scope and M/S monitoring controls.
  - Single-band or multi-band correlation meter.
  - Adjustable correlation window.
  - Lissajous, dot-cloud, and frequency-band dot-cloud modes.
  - Adjustable scale, rotation, channel flip, and grid overlay.
- **Waveform**
  - Left, right, and summed-channel views.
  - Adjustable scroll speed.
  - Peak-history overlay.
  - Color by spectral content, loudness, or a static color.

## Installation

### Prerequisites

OpenMeters requires:

1. A graphical Linux session on X11 or Wayland.
2. PipeWire installed and running.
3. Vulkan support through your distribution's Vulkan loader and driver
   stack.
4. **Pre-built** packages currently target `glibc` >= v2.41 (Debian 13
   or later, Fedora 44 or later).

Wayland is required for bar mode. Normal application windows are
available on both X11 and Wayland.

### Arch Linux

Install the `openmeters-git` package from the AUR:

```bash
yay -S openmeters-git
```

### Debian

Download the latest `.deb` package from [GitHub
Releases](https://github.com/httpsworldview/openmeters/releases).

### Fedora

Download the latest `.rpm` package from [GitHub
Releases](https://github.com/httpsworldview/openmeters/releases).

### Other distributions

Tarballs are available under tagged releases, or you can build
OpenMeters from source. I cannot guarantee OpenMeters will work on
every distribution, although it is designed to stay fairly
distro-agnostic. If you have trouble getting it running, please open
an issue and I will try to help.

### Building from source

1. Install PipeWire, PipeWire development headers, Vulkan, and a Rust
   toolchain. The recommended way to install Rust is
   [rustup](https://rustup.rs/). OpenMeters currently requires the
   Rust version declared in `Cargo.toml` or newer.
2. Clone the repository:

   ```bash
   git clone https://github.com/httpsworldview/openmeters/
   cd openmeters
   ```

3. Build and run the release binary:

   ```bash
   cargo build --release
   ./target/release/openmeters
   ```

   Or run it directly through Cargo:

   ```bash
   cargo run --release
   ```

#### Packaging

See [`packaging/`](./packaging/) for instructions on building Debian,
Fedora, and tarball artifacts.

## Usage and key bindings

### Global

| Binding | Action |
| --- | --- |
| `ctrl+shift+h` | Show or hide the global configuration drawer. |
| `p` | Pause or resume rendering. |
| `q` twice | Quit the application. |
| `ctrl+space` | Pop out or dock the hovered visual. |

### Spectrogram

| Binding | Action |
| --- | --- |
| `ctrl+scroll up/down` | Zoom vertically. |
| `middle click+drag` | Pan vertically. |

## Configuration

Application settings are saved to:

```text
~/.config/openmeters/settings.json
```

`settings.json` is intentionally editable. GUI ranges are not hard
limits, and unsupported keys or structurally invalid values are logged
and ignored at the narrowest practical scope.

Invalid JSON syntax is ignored and default settings are used instead.
Your configuration file will not be overwritten unless you change
settings in the GUI. The settings schema is intended to remain
compatible; any changes will be documented or migrated.

If a bug causes OpenMeters to misbehave, you can reset application
settings by deleting `settings.json`. Please consider reporting the
bug if you run into this.

### Theming

Themes are saved as separate JSON files in:

```text
~/.config/openmeters/themes/
```

You can create and switch between themes in the **Theme** tab of the
configuration page. Saving a theme refreshes the list of available
themes, including any files that appeared in the theme directory while
OpenMeters was running. The built-in theme is read-only and cannot be
overwritten or deleted. Feel free to share custom themes by sharing
the corresponding JSON files.

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on how to
contribute and how to get started. No matter what you have to offer, I
greatly appreciate your interest in the project. Every bit helps.

### Reuse and license

OpenMeters is GPL-3.0-or-later software. You may reuse code under the
GPL's terms. If you wan tto discuss other arrangements or have
licensing questions, feel free to email me at
<httpworldview@gmail.com>. You are also free to draw inspiration from
the ideas and methods used here, provided you respect the GPL.

It would be unfair to ask for attribution over ideas alone when this
project exists thanks to the countless researchers and open-source
contributors who came before me.

## Credits

Thank you for checking out OpenMeters. If you think it is useful,
please consider starring the repository and sharing it with others. I
appreciate criticism, bug reports, and feedback, so feel free to reach
out.

### Projects

- **MiniMeters** (<https://minimeters.app/>) inspired this project. If
  you can, please support their work.
- **EasyEffects** (<https://github.com/wwmm/easyeffects>) has been a
  valuable reference, especially for the virtual-sink approach to
  per-application capture.
- **Ardura's Scrolloscope** (<https://github.com/ardura/Scrollscope>)
- **Tim Strasser's Oszilloskop**
  (<https://github.com/timstr/oszilloskop>)
- **Audacity** (<https://www.audacityteam.org/>)

### Papers and Standards

- ITU-R BS.1770-5, loudness and true peak measurement.
- IEC 61672-1, A-weighting reference curve.
- A. de Cheveigné and H. Kawahara, "YIN, a fundamental frequency
  estimator for speech and music".
- F. Auger and P. Flandrin, "Improving the readability of
  time-frequency and time-scale representations by the reassignment
  method".
- K. Kodera, R. Gendrin, and C. de Villedary, "Analysis of
  time-varying signals with small BT values".
- S. Fulop and K. Fitz, "Algorithms for computing the time-corrected
  instantaneous frequency (reassigned) spectrogram, with
  applications".

### Libraries

- **iced_layershell** and related crates
  (<https://github.com/waycrate/exwlshelleventloop>)
  - Special thanks to Decodetalkers for reviewing and merging my
    patches, and for maintaining such a useful library.
- **Iced** (<https://github.com/iced-rs/iced>)
- **RustFFT** (<https://github.com/ejmahler/RustFFT>)
- **RealFFT** (<https://github.com/HEnquist/realfft>)
- **wgpu** (<https://github.com/gfx-rs/wgpu>)

## License

OpenMeters is licensed under the GNU General Public License v3.0 or
later. See [LICENSE](LICENSE) for more details.
