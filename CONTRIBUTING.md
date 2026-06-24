# Welcome

I greatly appreciate you taking the time to read this. Throughout
development, it has always felt as though I was building this project
in a vacuum. Often it feels as though I'm the only person who will
ever use this software, and that all this is doing is talking to
myself. So, thank you for being here. I hope you find this software
useful, and if you have any feedback or suggestions, please don't
hesitate to reach out.

## What is OpenMeters?

OpenMeters is, at its core, the code within
`src/visuals/<name>/processor.rs`. The quality, rigor, and correctness
of that code is the most important part of this entire project. To
illustrate that importance I should be able to gesture towards any of
the existing visuals, but in short:

1. The spectrogram faithfully implements linear, log, and ERB
   frequency scaling, and most importantly Auger and Flandrin's method
   of spectral reassignment as described in their 1995 paper.
2. The spectrum analyzer implements A-weighting according to IEC
   61672:2003, and shares the same linear, log, and ERB frequency
   scaling.
3. Although there is no direct standard for generating audio waveforms
   (as far as I am aware), our waveform's implementation takes
   inspiration from Audacity and Chris Needham's `audiowaveform`.
4. The oscilloscope uses normalized autocorrelation period estimation
   and waveform-template correlation to keep traces stable across
   complex periodic signals and visible channel selections.
5. The stereometer separates bands using LR4 butterworth filters,
   along with linear and log scaling. The correlation meter uses those
   same Butterworth crossings.
6. The loudness meter implements K-weighting relative to full
   scale/LUFS momentary/short-term, True Peak, and RMS
   fast/slow. Standards used include ITU-R BS.1770.

The point of this section is not to discourage engagement with the
project, rather it is to emphasize the expected level at which
contributions are to operate.

## Licensing information

OpenMeters is licensed under the GNU General Public License version
3.0 or later. See the [LICENSE](LICENSE) file for more information,
but you knew that already. Code contributed to this project is also
licensed under GPL-3.0-or-later, but the contributor retains copyright
to their contributions.

Project-owned Rust files generally include an SPDX license header for
GPL-3.0-or-later. Follow the nearby file style. If you adapt
third-party code, keep the required notice and license information
with the code and update packaged license documentation when needed.

## Commit message format

Try to follow the general format of:

```text
<type>(<scope>): <subject>
<body>
<DCO>
```

Where:

- `<type>` is a noun describing the type of change, such as "fix",
  "feat", "refactor", etc.
- `<scope>` is an optional noun describing the area of the codebase
  affected by the change, or a specific component or module. This is
  optional, but can be helpful for understanding the context of the
  change.
- `<subject>` is a short description of the change. Save the details
  for the body.
- `<body>` is an optional longer description of the change, which can
  include motivation, implementation, etc.
- `<DCO>` is a `Signed-off by:` footer certifying that you have the
  right to submit the contribution under this project's license. Use
  git's `--signoff` (`-s`) flag to add it automatically:
  
  ```bash
  $ git commit -s -m "fix(spectrum): correct a-weighting calculation"
  ```

## Development environment

OpenMeters is a Rust 2024 project. Use the Rust version declared in
`Cargo.toml` (`rust-version`) or newer. CI currently uses Rust 1.95
with `rustfmt` and `clippy` installed.

```bash
rustup toolchain install 1.95
rustup component add rustfmt clippy --toolchain 1.95
```

You also need native development packages for PipeWire, Wayland/X11,
xkbcommon, fontconfig/freetype, libclang, pkg-config, and Vulkan.
PipeWire/SPA development headers must be from PipeWire 0.3.65 or newer;
Ubuntu 22.04's 0.3.48 headers are too old for the current Rust
bindings. On Ubuntu/Debian-like systems, the CI dependency set is:

```bash
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential \
  libclang-dev \
  libpipewire-0.3-dev \
  libwayland-dev \
  libx11-dev \
  libx11-xcb-dev \
  libxext-dev \
  libxfixes-dev \
  libxrandr-dev \
  libxcursor-dev \
  libxi-dev \
  libxinerama-dev \
  libxkbcommon-dev \
  libfontconfig1-dev \
  libfreetype6-dev \
  libvulkan-dev \
  pkg-config
```

Many of these come preinstalled, but if you run into issues, perhaps
that will help.

## Getting started

Fork and clone the repository, then create a topic branch:

```bash
git clone https://github.com/your-username/openmeters.git
cd openmeters
git switch -c my-change
```

Useful commands while iterating:

```bash
cargo check --workspace --locked
cargo run
cargo run --release
RUST_LOG=openmeters=debug cargo run
```

Development builds are faster and include debug information. For
performance work you may use the `profiling` profile, which is the
same as `release` except for the fact that it includes debug symbols.

## Verification before opening a pull request

The CI workflow currently runs these checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo test --workspace --locked --all-targets
```

Please run the same checks locally when practical. For
documentation-only changes, a full build is usually not necessary, but
please still check formatting, links, examples, and spelling as
appropriate.

For changes that affect runtime behavior, test:

- DSP/visual processors: test generated signals or known references
  and cover edge cases with unit tests.
- UI changes: run the app and check the affected
  panel/window/pop-out/bar/etc. behavior.
- PipeWire changes: verify capture, route enable/disable,
  default-device/etc. behavior, and clean shutdown.
- Settings changes: test loading existing settings, invalid values,
  unknown keys, saving through the UI, and theme interactions.
- Packaging changes: build the release binary first, then run the
  relevant `packaging/` target.

## Release tags

Release tags use `v<Cargo.toml package.version>` for normal upstream
releases. For packaging-only rebuilds, append a positive package
release suffix: `v1.7.1-2`. The suffix does not change the Rust crate
version; it is passed to `make -C packaging RELEASE=<suffix>` for the
Debian/RPM release number.

## Repository layout

```text
src/domain.rs              shared application enums and routing/visual identifiers
src/dsp.rs                 AudioBlock and small DSP types
src/util/                  low-level audio math, color, musical, and telemetry helpers
src/infra/pipewire/        PipeWire registry monitor, routing, virtual sink, tap.
src/persistence/           settings schema, lossy loading, store, themes, visual config
src/visuals.rs             visual module declarations, option enums, widget macros
src/visuals/               visual processors, state, render primitives, palettes, registry
src/visuals/render/        shared render helpers and WGSL shaders
src/ui/                    iced app, subscriptions, pages, widgets, theme, windowing
src/main.rs                application wiring and shutdown flow
packaging/                 Debian/RPM/tarball packaging things
```

Keep dependencies between these areas shallow and one-way where
possible:

- `domain`, `dsp`, and `util` should not depend on UI or PipeWire.
- `infra` should stay focused on PipeWire integration and routing.
- `persistence` owns serialized settings and theme file behavior.
- `visuals` owns processor/state/render code and the visual registry.
- `ui` composes application state, settings handles, subscriptions,
  windows, pages, and widgets.

## Working on visuals

Each visual lives under `src/visuals/<name>/`:

- `processor.rs` converts `AudioBlock`s into snapshots. This is the
  most important part to test carefully.
- `state.rs` owns user facing visual state and maps snapshots into
  render parameters.
- `render.rs` owns custom iced/wgpu drawing primitives.

When adding or changing a visual, also check the related wiring:

- `src/domain.rs` for `VisualKind`.
- `src/visuals.rs` for module declarations and option enums.
- `src/visuals/palettes.rs` for default palettes.
- `src/visuals/registry.rs` for descriptors, settings application,
  export, enablement, ordering, and sample delivery.
- `src/persistence/visuals.rs` for serializable visual settings and
  lossy parsing.
- `src/ui/pages/visuals/settings/` for settings panels.
- `README.md` if the user-visible behavior changes.

Use existing visuals as templates and always use shared render helpers
in `src/visuals/render/common.rs`; add new render code only when they
don't fit.

## Testing expectations

Add or update tests when a change has behavior that can
regress. Existing unit tests live beside the code they exercise.

Tests for this project usually:

- exercise public or module-level behavior rather than incidental
  implementation details;
- use generated audio signals, reference values, tolerances, and edge
  cases for DSP; and
- include a regression case for every bug fix that can be reproduced
  in a small test.

If a behavior is impractical to automate, describe the manual checks
you performed instead.

## Pull requests

Once you've made your changes, you may open a pull request. Ensure
that your PR includes a clear description of the changes you've made,
and any relevant context or motivation. If your changes are related to
an open issue, please link to it in the description. I will do my best
to review and merge your PR in a timely manner.

The title of your PR should follow the same format as commit messages,
in fact, feel free to use the same message for both your commit and PR
title. Use your best judgment.

Ultimately, I decide what gets merged, but I will never reject a PR
due to grammatical issues or petty semantics alone. I will work with
you to get it merged if I think the change is good for the project.
