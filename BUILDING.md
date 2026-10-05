# Building Xenovision

Everything, including ENVI/GeoTIFF/`.mat` image import, is pure Rust - no
native library dependency, no per-platform setup. Just a Rust toolchain
([rustup.rs](https://rustup.rs)):

```sh
cargo build --workspace
cargo test --workspace --release
cargo run -p xenovision-app
```

## Release packages

`scripts/package.sh` builds a package into `dist/`, with the user guide
(`README.md`) rendered to `user-guide.pdf` inside it:

```sh
scripts/package.sh              # this OS: Linux tarball or macOS .dmg
scripts/package.sh --windows    # Windows zip, cross-built from Linux
```

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
  `XDG_DATA_HOME` at a scratch directory to keep that out of your own one:
  `XDG_DATA_HOME=/tmp/xeno-test cargo test --workspace --release`.
- **GPU.** `xenovision-core` depends on `wgpu`, but nothing currently uses
  it at runtime (see `crates/xenovision-core/src/gpu.rs`); it needs no
  setup and no GPU.
