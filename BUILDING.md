# Building Xenovision

Everything needs only a Rust toolchain ([rustup.rs](https://rustup.rs))
except one feature: ENVI/GeoTIFF image import, which links the native GDAL
library. GDAL is an optional cargo feature (`gdal`), on by default, so you
can build either way:

| | Needs GDAL installed | ENVI/GeoTIFF import |
|---|---|---|
| Default build | yes | yes |
| `--no-default-features` | no | no (MATLAB `.mat` and text/CSV import still work) |

## Without GDAL

```sh
cargo run -p xenovision-app --no-default-features
cargo test -p xenovision-core -p xenovision-app --no-default-features
```

Name the two crates rather than using `--workspace`: the workspace also
contains `spikes/spike_b`, a standalone GDAL experiment that depends on GDAL
directly regardless of the feature.

## With GDAL (the default)

Install GDAL's development files, then build as usual:

```sh
cargo build --workspace
cargo test --workspace --release
cargo run -p xenovision-app
```

GDAL's shared library (and its PROJ/GEOS dependencies) must also be present
on any machine that *runs* a GDAL-enabled build.

### Linux

`gdal-sys` finds GDAL through `pkg-config`:

```sh
sudo apt install libgdal-dev      # Debian/Ubuntu
sudo dnf install gdal-devel       # Fedora
sudo pacman -S gdal               # Arch
```

### macOS

```sh
brew install gdal pkg-config
```

### Windows

`gdal-sys` finds GDAL through the `GDAL_HOME` environment variable (or
`GDAL_INCLUDE_DIR`/`GDAL_LIB_DIR`). The simplest source is
[vcpkg](https://vcpkg.io):

```powershell
vcpkg install gdal:x64-windows
$env:GDAL_HOME = "<path-to-vcpkg>\installed\x64-windows"
cargo build --workspace
```

At runtime `gdal.dll` and its dependency DLLs (in that prefix's `bin`
folder) must be findable: put that folder on `PATH`, or copy the DLLs next
to the exe. A build that links fine can still fail to *launch* without
them. OSGeo4W or conda-forge (`conda install -c conda-forge gdal`) work as
alternative sources for the same `include`/`lib`/`bin` layout.

## Release packages

`scripts/package.sh` builds a package into `dist/`, with the user guide
(`docs/user-guide.md`) rendered to `user-guide.pdf` inside it:

```sh
scripts/package.sh              # this OS: Linux tarball or macOS .dmg
scripts/package.sh --windows    # Windows zip, cross-built from Linux
scripts/package.sh --with-gdal  # include ENVI/GeoTIFF import (adds "-gdal" to the name)
```

Packages are built **without GDAL** by default, so they run on a stock
system with nothing else installed. A `--with-gdal` package needs GDAL on
the machine that runs it.

| Package | Built on | Contents |
|---|---|---|
| `xenovision-<version>-linux-<arch>.tar.gz` | Linux | `xenovision` binary, `user-guide.pdf` |
| `xenovision-<version>-windows-x86_64.zip` | Linux (`--windows`) | `Xenovision.exe`, `user-guide.pdf` |
| `xenovision-<version>-macos-<arch>.dmg` | macOS | `Xenovision.app` (PDF also in its Resources), `user-guide.pdf` |

### Tools needed

- **Every package:** `pandoc` and `wkhtmltopdf`, for the PDF. On Linux:
  `apt install pandoc wkhtmltopdf`. On macOS: `brew install pandoc`, plus
  wkhtmltopdf from its final release at
  [wkhtmltopdf.org/downloads.html](https://wkhtmltopdf.org/downloads.html)
  (the project is archived, so Homebrew may no longer carry it). Styling
  comes from `docs/user-guide-print.css`.
- **Windows (`--windows`):** the MinGW cross-compiler and Rust's Windows
  target:
  ```sh
  sudo apt install mingw-w64
  rustup target add x86_64-pc-windows-gnu
  ```
  `python3` is used to write the zip. The resulting exe imports only DLLs
  that ship with Windows, so it runs as a single file.
- **macOS:** nothing extra (`hdiutil` and `codesign` are built in).

### Platform notes

- **Windows can't be built natively by this script**; cross-build it from
  Linux (or WSL) with `--windows`. Release builds use the Windows GUI
  subsystem, so no console window opens; debug builds keep the console for
  log output.
- **macOS can only be built on a Mac**: cross-compiling needs Apple's SDK,
  which its license restricts to Apple hardware. The app is ad-hoc signed,
  not signed with a Developer ID or notarized, so the first launch needs
  right-click → Open. Proper signing needs an Apple Developer account.

## Notes

- **Test data location.** The app's headless layout tests start the app
  normally, which bootstraps the built-in species fixtures into your
  per-user data directory (`xenovision/fixtures/`). On Linux, point
  `XDG_DATA_HOME` at a scratch directory to keep that out of your real one:
  `XDG_DATA_HOME=/tmp/xeno-test cargo test --workspace --release`.
- **GPU.** `xenovision-core` depends on `wgpu`, but nothing currently uses
  it at runtime (see `crates/xenovision-core/src/gpu.rs`); it needs no
  setup and no GPU.
