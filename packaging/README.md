# Packaging

Mostly everything needed to produce distribution packages for
OpenMeters is contained within this directory.

Arch users are served by the AUR (`openmeters-git`), so `.pkg.tar.zst`
is not built here.

## Requirements

- `cargo` (to produce the release binary, invoked separately)
- [`nfpm`](https://nfpm.goreleaser.com/)
- `git`, `rustc`, `objdump`, `tar`, `xz`, `sha256sum`, `awk`

## Files

- `Makefile` - driver.
- `nfpm.yaml` - deb/rpm spec.
- `copyright` - Debian copyright/license metadata, also shipped in
  tarballs.

## Building

**Build the release binary first from the repo root.** The Makefile
only assembles artifacts.

```bash
cargo build --locked --release # from repo root
cd packaging

make check        # print the package ABI/dependency floor
make MAX_GLIBC_VERSION=2.39 check
make tarball      # dist/openmeters-<version>-x86_64-linux-gnu.tar.xz
make deb          # dist/openmeters_<version>-1_amd64.deb
make rpm          # dist/openmeters-<version>-1.x86_64.rpm
make RELEASE=2 all # package rebuild: deb/rpm release number 2
make all          # all of the above, plus SHA256SUMS
make clean        # wipe dist/
```

Version is parsed from the root `Cargo.toml`. `RELEASE` defaults to
`1` and is passed to nFPM for Debian/RPM package rebuilds. Run nFPM
through `make`. `MAX_GLIBC_VERSION` is optional for local builds, but
CI sets it so official artifacts cannot silently move to a newer glibc
floor.

## Artifact paths (example version)

```
dist/
  openmeters-<version>-x86_64-linux-gnu/      staging tree for the tarball
  openmeters-<version>-x86_64-linux-gnu.tar.xz
  openmeters_<version>-<release>_amd64.deb
  openmeters-<version>-<release>.x86_64.rpm
  SHA256SUMS
```

`dist/` is gitignored.

## Runtime dependencies

- `glibc` >= 2.39 for pre-built release artifacts. Local packages
  declare the highest `GLIBC_*` symbol required by the built binary.
- `libgcc_s.so.1`
- `libpipewire-0.3.so.0` >= 0.3.65 (audio I/O and virtual sink)
- `libvulkan.so.1` (wgpu uses the distro's Vulkan loader + ICDs)
- Wayland: `libwayland-client.so.0`
- X11: `libX11.so.6`, `libX11-xcb.so.1`, `libxcb.so.1`,
  `libXcursor.so.1`, `libXi.so.6`
- Keyboard input: `libxkbcommon.so.0`, `libxkbcommon-x11.so.0`

Some graphics/windowing libraries are loaded with `dlopen`, so they
may not appear in `readelf -d` output.

## Tarball layout

```
openmeters-<version>-x86_64-linux-gnu/
  bin/openmeters
  share/applications/openmeters.desktop
  share/icons/hicolor/scalable/apps/openmeters.svg
  share/doc/openmeters/copyright
  share/licenses/openmeters/LICENSE
  share/licenses/openmeters/iced_widget_pane_grid.md
  LICENSE
  README.md
```
