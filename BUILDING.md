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

On Windows itself, `scripts/package.ps1` builds the Windows zip natively (no
MinGW cross-compile), with the same layout and download name:

```powershell
powershell -File scripts\package.ps1        # build + PDF + zip
powershell -File scripts\package.ps1 -SkipPdf  # binary-only; no pandoc needed
```

From Windows, the Linux tarball can also be built through WSL (which keeps its
build artifacts on the Linux filesystem, never in the repo's `target/`):

```powershell
powershell -File scripts\package-linux.ps1              # default distro
powershell -File scripts\package-linux.ps1 -Distro Ubuntu-26.04 -TargetDir /home/me/xeno-target
```

| Package | Built on | Contents |
|---|---|---|
| `xenovision-<version>-linux-<arch>.tar.gz` | Linux, or WSL from Windows (`package-linux.ps1`) | `xenovision` binary, `user-guide.pdf` |
| `xenovision-<version>-windows-x86_64.zip` | Linux (`--windows`) or native Windows (`package.ps1`) | `Xenovision.exe`, `user-guide.pdf` (exe only with `-SkipPdf`) |
| `xenovision-<version>-macos-<arch>.dmg` | macOS | `Xenovision.app` (PDF also in its Resources), `user-guide.pdf` |

### Tools needed

- **Every package:** `pandoc` and `wkhtmltopdf`, for the PDF. On Linux:
  `apt install pandoc wkhtmltopdf`. On macOS: `brew install pandoc`, plus
  wkhtmltopdf from its final release at
  [wkhtmltopdf.org/downloads.html](https://wkhtmltopdf.org/downloads.html)
  (the project is archived, so Homebrew may no longer carry it). On Windows:
  `winget install JohnMacFarlane.Pandoc wkhtmltopdf.wkhtmltopdf`. Styling
  comes from `docs/user-guide-print.css`.
- **Windows (`--windows`):** the MinGW cross-compiler and Rust's Windows
  target:
  ```sh
  sudo apt install mingw-w64
  rustup target add x86_64-pc-windows-gnu
  ```
  `python3` is used to write the zip. The resulting exe imports only DLLs
  that ship with Windows, so it runs as a single file.
- **Windows (native `package.ps1`):** only cargo, plus `pandoc`/
  `wkhtmltopdf` if you want the PDF included (`-SkipPdf` skips both the PDF
  and its tool checks). If `llvm-strip` is on PATH (`rustup component add
  llvm-tools-preview`) the exe is stripped; otherwise it ships unstripped
  with a warning. The zip is written with PowerShell's `Compress-Archive`.
  Without admin rights, install both tools portably: extract pandoc's
  `pandoc-<v>-windows-x86_64.zip` and wkhtmltopdf's
  `wkhtmltox-0.12.6-1.mxe-cross-win64.7z` into any user-writable folder and
  add their `bin` directories to the user PATH. `package.ps1` re-merges the
  registry Machine+User PATH at startup, so it finds freshly installed tools
  even in processes started before the PATH change.
- **Windows (Linux package via WSL, `package-linux.ps1`):** WSL with a
  distro that has `cargo`, `pandoc`, `wkhtmltopdf` (`sudo apt install cargo
  pandoc wkhtmltopdf`). The script runs `package.sh` inside the distro with
  `CARGO_TARGET_DIR` on the Linux filesystem.
- **macOS:** nothing extra (`hdiutil` and `codesign` are built in).

### Platform notes

- **Windows:** `package.sh` can only cross-build the zip from Linux (it is a
  bash script, and needs MinGW); `package.ps1` builds it natively on Windows
  with the regular MSVC Rust toolchain. Both produce the same single-file
  exe. Release builds use the Windows GUI subsystem, so no console window
  opens; debug builds keep the console for log output.
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
