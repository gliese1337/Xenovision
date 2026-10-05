#!/usr/bin/env bash
# Builds a release package of Xenovision with the user guide as a PDF.
#
#   scripts/package.sh              package for this OS (Linux or macOS)
#   scripts/package.sh --windows    cross-build the Windows package (from Linux)
#
# Output goes to dist/. Needs: cargo, pandoc, wkhtmltopdf. --windows also
# needs the x86_64-pc-windows-gnu Rust target and mingw-w64; macOS needs
# hdiutil (built in).
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"

WINDOWS=0
for arg in "$@"; do
  case "$arg" in
    --windows) WINDOWS=1 ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg (see --help)" >&2; exit 2 ;;
  esac
done

need() {
  command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1 ($2)" >&2; exit 1; }
}
need cargo "install Rust from https://rustup.rs"
need pandoc "e.g. apt install pandoc / brew install pandoc"
need wkhtmltopdf "e.g. apt install wkhtmltopdf / brew install --cask wkhtmltopdf"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/xenovision-app/Cargo.toml | head -1)
DIST="$ROOT/dist"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$DIST"

echo "==> Rendering user guide PDF"
{ echo "<style>"; cat docs/user-guide-print.css; echo "</style>"; } > "$WORK/head.html"
pandoc README.md -o "$WORK/user-guide.pdf" \
  --pdf-engine=wkhtmltopdf \
  --metadata pagetitle="Xenovision User Guide" \
  --include-in-header="$WORK/head.html" \
  -V margin-top=18mm -V margin-bottom=18mm -V margin-left=18mm -V margin-right=18mm \
  2> >(grep -vE "QStandardPaths|Loading pages|Printing pages|Done|\[=" >&2 || true)

if [ "$WINDOWS" -eq 1 ]; then
  [ "$(uname -s)" = Linux ] || { echo "--windows cross-builds from Linux only" >&2; exit 1; }
  need x86_64-w64-mingw32-strip "apt install mingw-w64"
  need python3 "used to write the zip"
  TARGET=x86_64-pc-windows-gnu
  rustup target list --installed 2>/dev/null | grep -qx "$TARGET" \
    || { echo "missing Rust target: rustup target add $TARGET" >&2; exit 1; }
  echo "==> Building for Windows ($TARGET)"
  cargo build --release -p xenovision-app --target "$TARGET"
  NAME="xenovision-$VERSION-windows-x86_64"
  mkdir -p "$WORK/$NAME"
  cp "target/$TARGET/release/xenovision-app.exe" "$WORK/$NAME/Xenovision.exe"
  x86_64-w64-mingw32-strip "$WORK/$NAME/Xenovision.exe"
  cp "$WORK/user-guide.pdf" "$WORK/$NAME/"
  (cd "$WORK" && python3 -m zipfile -c "$DIST/$NAME.zip" "$NAME")
  echo "==> $DIST/$NAME.zip"
  exit 0
fi

echo "==> Building for $(uname -s)"
cargo build --release -p xenovision-app
BIN="target/release/xenovision-app"

case "$(uname -s)" in
  Linux)
    NAME="xenovision-$VERSION-linux-$(uname -m)"
    mkdir -p "$WORK/$NAME"
    cp "$BIN" "$WORK/$NAME/xenovision"
    strip "$WORK/$NAME/xenovision"
    cp "$WORK/user-guide.pdf" "$WORK/$NAME/"
    tar -C "$WORK" -czf "$DIST/$NAME.tar.gz" "$NAME"
    echo "==> $DIST/$NAME.tar.gz"
    ;;
  Darwin)
    need hdiutil "built into macOS"
    NAME="xenovision-$VERSION-macos-$(uname -m)"
    APP="$WORK/$NAME/Xenovision.app"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    cp "$BIN" "$APP/Contents/MacOS/Xenovision"
    strip "$APP/Contents/MacOS/Xenovision"
    cp "$WORK/user-guide.pdf" "$APP/Contents/Resources/"
    cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Xenovision</string>
  <key>CFBundleDisplayName</key><string>Xenovision</string>
  <key>CFBundleIdentifier</key><string>org.xenovision.app</string>
  <key>CFBundleExecutable</key><string>Xenovision</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
    # Unsigned: without an Apple Developer ID, Gatekeeper blocks a plain
    # double-click; users right-click > Open the first time. Ad-hoc
    # signing at least lets it run on Apple Silicon.
    codesign --force --deep --sign - "$APP" 2>/dev/null || true
    # The PDF also sits next to the app in the disk image, where users see it.
    cp "$WORK/user-guide.pdf" "$WORK/$NAME/"
    hdiutil create -quiet -volname "Xenovision $VERSION" -srcfolder "$WORK/$NAME" \
      -ov -format UDZO "$DIST/$NAME.dmg"
    echo "==> $DIST/$NAME.dmg"
    ;;
  *)
    echo "unsupported OS: $(uname -s) - on Windows, build from Linux with --windows" >&2
    exit 1
    ;;
esac
