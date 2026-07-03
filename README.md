# OpenMeters

![https://github.com/user-attachments/assets/52d0202e-f6e7-47aa-9952-e3a0be975f42](https://github.com/user-attachments/assets/e7b15cd0-eb12-4e99-b036-79b11e04bb46)

An open-source audio metering and visualization suite for Linux,
designed for enthusiasts, professionals, and everyone in between.

## Quick links

- [Features](#features)
- [Installation](#installation)
  - [Arch Linux](#arch-linux)
  - [Debian](#debian)
  - [Fedora](#fedora)
  - [NixOS](#nixos)
  - [Build from source](#building-from-source)
- [Usage and key bindings](#usage-and-key-bindings)
- [Configuration and theming](#configuration)
- [Contributing](#contributing)
- [Credits](#credits)
- [Notes](#notes)
- [License](#license)

## Features

Everything below describes behavior implemented thus far. If you have
ideas for the next thing OpenMeters should do, please feel free to
open an issue or pull request.

### General

- PipeWire audio capture
  - Per-application capture through a PipeWire virtual sink.
  - Device/default-sink capture.
  - Application routes touched by OpenMeters are reset on clean
    shutdown.
- Windowing
  - Normal desktop windows on X11 or Wayland.
  - Wayland layer-shell bar mode when the compositor exposes
    `zwlr_layer_shell_v1`, anchored to the top or bottom of a given
    monitor.
  - Pop-out windows for individual visuals.
  - Window decoration toggle.
- Appearance and persistence
  - Configurable RGBA background color.
  - Editable JSON settings with lossy loading for unknown or invalid
    fields.
  - User themes.

### Visuals

- **Loudness**
  - BS.1770-5 K-weighted short-term and momentary LUFS meter modes.
  - True Peak meter modes.
  - Fast and slow K-weighted RMS dB meter modes.
- **Oscilloscope**
  - Selectable left, right, mid/mono, side, or `none` channel traces.
  - Selectable trigger source, including channel-dependent triggering
    for independently stable traces.
  - A stable waveform trigger that uses autocorrelation period
    estimation and waveform template correlation. (Often referred to
    as "pitch-following" or "phase-locking" in other applications.)
  - Selectable cycle count in stable trigger mode.
  - Zero-crossing trigger for traditional scope behavior.
- **Spectrogram**
  - A multitude of window types, lengths, and hop sizes.
  - Classic STFT rendering.
  - Time-frequency reassignment (Similar to Wavecandy's "Enhanced
    frequency" mode, or MiniMeters' "Sharper" mode.)
  - Click-and-hold crosshair with frequency, note, and time tooltip.
  - Piano-roll overlay.
  - Frequency-axis zoom and pan.
  - ERB, logarithmic, and linear frequency scales.
  - Adjustable color map, stop positions, and stop spreads.
- **Spectrum analyzer**
  - A multitude of window types, lengths, and hop sizes.
  - Selectable primary and secondary source: left, right, mid, side, or none.
  - Raw or IEC 61672-1 A-weighted display.
  - Peak label with frequency, note, and level.
  - No averaging, exponential averaging, or peak hold.
  - ERB, logarithmic, and linear frequency scales.
  - Line or bar display with adjustable color map.
- **Stereometer**
  - L/R vector display in Lissajous or dot-cloud modes.
  - Frequency-band dot-cloud mode with low/mid/high bands.
  - Single-band or low/mid/high phase-correlation meter.
  - Adjustable correlation window.
  - Adjustable dot-cloud scale, rotation, channel flip, unipolar fold,
    dot size, and grid.
- **Waveform**
  - Selectable left, right, mid/mono, side, or `none` channel lanes.
  - Adjustable scroll speed.
  - Optional low/mid/high band-level history overlay.
  - Color by low/mid/high band balance, loudness, or a static color.

## Installation

### Prerequisites

OpenMeters requires:

1. A graphical Linux session on X11 or Wayland.
2. PipeWire installed and running.
3. Vulkan support through your distribution's Vulkan loader and driver
   stack.
4. For pre-built release artifacts: x86_64 GNU/Linux with `glibc` >=
   v2.39. The release workflow builds these artifacts in Ubuntu 24.04.

Normal application windows are available on both X11 and Wayland. Bar
mode additionally requires a Wayland compositor that exposes
`zwlr_layer_shell_v1`.

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

### NixOS

OpenMeters is available in `nixpkgs` thanks to
[@magnetophon](https://github.com/magnetophon) and
[@bitbloxhub](https://github.com/bitbloxhub). Add it to your nix
config with:

```bash
nix profile add nixpkgs#openmeters
```

### Other distributions

Tarballs are available under tagged releases, or you can build
OpenMeters from source. I cannot guarantee OpenMeters will work on
every distribution, although it is designed to stay fairly
distro-agnostic. If you have trouble getting it running, please open
an issue and I will try to help.

### Building from source

1. Install a Rust toolchain, a C toolchain, `pkg-config`, `libclang`,
   and native development packages for PipeWire, Wayland/X11,
   xkbcommon, fontconfig/freetype, and the Vulkan loader/development
   headers. PipeWire/SPA development headers must be from PipeWire
   0.3.65 or newer. The recommended way to install Rust is
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
| `ctrl+shift+h` | Show/hide the configuration window; while open, drag visuals to rearrange them. |
| right click on a visual | Open that visual's settings window. |
| `p` | Pause or resume meter updates. |
| `q` twice | Quit the application. |
| `ctrl+space` | Pop out the hovered visual, or dock the focused pop-out. |

### Spectrogram

| Binding | Action |
| --- | --- |
| left click+hold | Show the crosshair and frequency/note/time tooltip. |
| `ctrl+scroll up/down` | Zoom the frequency axis. |
| `middle click+drag` | Pan the frequency axis. |

## Configuration

Application settings are saved to
`$XDG_CONFIG_HOME/openmeters/settings.json`, or to:

```text
~/.config/openmeters/settings.json
```

when `XDG_CONFIG_HOME` is unset.

`settings.json` is intentionally editable. GUI ranges are not hard
limits; processors normalize only the values they need for safe
runtime behavior. Unsupported keys or structurally invalid values are
logged and ignored at the narrowest practical scope.

Invalid JSON syntax is ignored and default settings are used for that
run. Your configuration file will not be overwritten unless you change
settings in the GUI. Unknown keys are not preserved when the file is
next written.

If a bug causes OpenMeters to misbehave, you can reset application
settings by deleting `settings.json`. Please consider reporting the
bug if you run into this.

### Theming

Themes are saved as separate JSON files in
`$XDG_CONFIG_HOME/openmeters/themes/`, or in:

```text
~/.config/openmeters/themes/
```

when `XDG_CONFIG_HOME` is unset. Theme files own palettes and
background color; `settings.json` stores the selected theme name and
non-palette module settings.

You can create and switch between themes in the **Theme** tab of the
configuration page. Saving a theme refreshes the list of available
themes, including any files that appeared in the theme directory while
OpenMeters was running. The built-in theme is read-only in the UI and
cannot be overwritten. Feel free to share custom themes by sharing the
corresponding JSON files.

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on how to
contribute and how to get started. No matter what you have to offer, I
greatly appreciate your interest in the project. Every bit helps.

### Reuse and license

OpenMeters is GPL-3.0-or-later software. You may reuse code under the
GPL's terms. If you want to discuss other arrangements or have
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
- **Corrscope** (<https://github.com/corrscope/corrscope>) was a key
  reference for correlation-triggered oscilloscope stability.
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
  - Historically, the algorithm within this paper was implemented by
    our oscilloscope.
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

## Notes

### FFT size convention

**OpenMeters labels FFT/window size as the actual number of input
samples used by the transform**.

Some other applications, including MiniMeters and Wave Candy, label
the visible positive-frequency bins instead. This is because for
real-valued audio, only the 0 Hz..Nyquist half of the FFT is
unique.

If you are matching settings from MiniMeters or Wave Candy, use
approximately double their displayed band count as the FFT size within
OpenMeters' GUIs (e.g. MiniMeters' 2048 = OpenMeters' 4096).

## License

OpenMeters is licensed under the GNU General Public License v3.0 or
later. See [LICENSE](LICENSE) for more details.
