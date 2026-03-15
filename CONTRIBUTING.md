# Welcome

I greatly appreciate you taking the time to read this. Throughout
development, it has always felt as though I was building this project
in a vacuum. Often it feels as though I'm the only person who will
ever use this software, and that all this is doing is talking to
myself. So, thank you for being here. I hope you find this software
useful, and if you have any feedback or suggestions, please don't
hesitate to reach out.

## Licensing information

OpenMeters is licensed under the GNU General Public License version
3.0 or later. See the [LICENSE](LICENSE) file for more information,
but you knew that already. Code contributed to this project is also
licensed under GPLv3, but the contributor retains copyright to their
contributions.

## Commit message format

There are no strict rules for commit messages, but I try to follow the
general format of:

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
- `<DCO>` is the Developer Certificate of Origin, a sign-off
  indicating that the contributor agrees to the terms of the
  license. Use git's `--signoff` (`-s`) flag to automatically add this
  to your commit messages (e.g. `git commit -s -m "chore: bump
  dependencies"`).

## Quick start - GitHub

Setting up a development environment is pretty straightforward. Make
sure you have Rust installed, fork & clone the repository, then poke
around a bit. In general, your workflow should look something like
this:

```bash
git clone https://github.com/your-username/openmeters.git
cd openmeters
git checkout -b my-awesome-thing
```

Make a change, then consider running the following (in no particular
order):

```bash
cargo check          # verify everything compiles
cargo test           # run the test suite
cargo clippy         # lint
cargo run            # build & run in dev mode to test your changes*
cargo fmt            # format your code
```

(*dev builds will be a tad slower, but they build orders of magnitude
faster and contain debug symbols, framepointers, etc. that are helpful
during development)

That's it. If `cargo clippy` passes, you're good to go.

## Architecture

Our codebase is organized into a handful of layers with a strict
dependency direction:

```text
domain/         - core types (VisualKind, RoutingConfig, CaptureMode)
dsp.rs          - FFT, windowing, generic DSP utilities
util/           - audio math, color utilities, tracing setup
infra/          - PipeWire integration (registry, monitor, virtual sink, sample tap)
persistence/    - settings schema, JSON store
visuals/        - per-visual processors, state machines, and wgpu render primitives
ui/             - iced application, pages, theme, widgets
main.rs         - orchestrator, launches the app
```

For the most part dependencies flow downward: `ui` can reach into
everything, `visuals` depends on `dsp`/`persistence`/`util`, `infra`
depends on `domain`, and `domain`/`dsp`/`util` depend on nothing
internal.

Each visual (spectrum, spectrogram, loudness, oscilloscope,
stereometer, waveform) follows the same three-file pattern under
`visuals/<name>/`:

- `processor.rs` - DSP processing
- `state.rs` - visual state machine
- `render.rs` - render primitive

## Where to start

- **Adding a new visual?** Use any existing visual directory
  (e.g. `visuals/spectrum/`) as a template. The `visuals!`,
  `vis_processor!`, and `visualization_widget!` macros handle the
  boilerplate, please use them.
- **UI changes?** Pages live in `ui/pages/`. The main app struct is in
  `ui/app.rs`.
- **PipeWire plumbing?** Everything lives under `infra/pipewire/`.
- **Settings?** Schema types are in `persistence/settings/schema.rs`,
  persisted via `persistence/settings/store.rs`.
- **Shaders?** WGSL files are in `visuals/render/shaders/`.

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
