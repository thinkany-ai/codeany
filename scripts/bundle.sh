#!/bin/sh
# Build a debug (or `release`) app bundle at the requested path.
set -e
APP=${2:-}
cd "$(dirname "$0")/.."
PROFILE=${1:-debug}
if [ "$PROFILE" = release ]; then
    cargo build --release
    ICON=codeany
    BUNDLE_ID=ai.codeany.desktop
    NAME=CodeAny
else
    cargo build
    ICON=codeany-dev
    # A separate identity so macOS never mixes the dev build up with the
    # installed app (Dock, `open`, permissions).
    BUNDLE_ID=ai.codeany.desktop.dev
    NAME="CodeAny Dev"
fi
APP=${APP:-target/codeany.app}
case "$APP" in
    *.app) ;;
    *) echo 'Expected an output path ending in .app' >&2; exit 1 ;;
esac
VERSION=$(cargo pkgid --locked | sed 's/.*[#@]//')
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$PROFILE/codeany" "$APP/Contents/MacOS/codeany"
cp "assets/$ICON.icns" "$APP/Contents/Resources/"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>codeany</string>
  <key>CFBundleIconFile</key><string>$ICON</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSLocalNetworkUsageDescription</key><string>CodeAny connects directly to your other devices on the same network, so using them is fast and stays off the internet.</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
PLIST
echo "$APP"
