# Packaging

Mostly everything needed to produce distribution packages for
OpenMeters is contained within this directory.

Arch users are served by the AUR (`openmeters-git`), so `.pkg.tar.zst`
is not built here.

## Requirements

- `cargo` (to produce the release binary, invoked separately)
- [`nfpm`](https://nfpm.goreleaser.com/)
- `tar`, `xz`, `sha256sum`, `awk`

## Files

- `Makefile` - driver.
- `nfpm.yaml` - deb/rpm spec.

## Building

**Build the release binary first from the repo root.** The Makefile
only assembles artifacts.

```bash
cargo build --release      # from repo root
cd packaging

make tarball    # dist/openmeters-<version>-x86_64-linux-gnu.tar.xz
make deb        # dist/openmeters_<version>-1_amd64.deb
make rpm        # dist/openmeters-<version>-1.x86_64.rpm
make all        # all of the above, plus SHA256SUMS
make clean      # wipe dist/
```

Version is parsed from the root `Cargo.toml`.

## Artifact paths (example version)

```
dist/
  openmeters-0.1.0-x86_64-linux-gnu/          staging tree for the tarball
  openmeters-0.1.0-x86_64-linux-gnu.tar.xz
  openmeters_0.1.0-1_amd64.deb
  openmeters-0.1.0-1.x86_64.rpm
  SHA256SUMS
```

`dist/` is gitignored.

## Runtime dependencies

- `libpipewire-0.3.so.0` (direct NEEDED; audio I/O and virtual sink)
- `libvulkan.so.1` (wgpu uses the distro's Vulkan loader + ICDs)
- `libwayland-client.so.0` (Wayland protocol)
- `libxkbcommon.so.0` (keyboard input)

(Note: if you find this list to be inaccurate please let me know.)

## Tarball layout

```
openmeters-<version>-x86_64-linux-gnu/
  bin/openmeters
  share/applications/openmeters.desktop
  share/icons/hicolor/scalable/apps/openmeters.svg
  LICENSE
  README.md
```
