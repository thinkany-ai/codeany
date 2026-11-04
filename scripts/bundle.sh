#!/bin/sh
# Build target/dscode.app from the current debug (or `release`) build.
set -e
cd "$(dirname "$0")/.."
PROFILE=${1:-debug}
if [ "$PROFILE" = release ]; then cargo build --release; else cargo build; fi
APP=target/dscode.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$PROFILE/dscode" "$APP/Contents/MacOS/dscode"
cp assets/dscode.icns "$APP/Contents/Resources/"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>dscode</string>
  <key>CFBundleDisplayName</key><string>dscode</string>
  <key>CFBundleIdentifier</key><string>cc.idoubi.dscode</string>
  <key>CFBundleExecutable</key><string>dscode</string>
  <key>CFBundleIconFile</key><string>dscode</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
PLIST
echo "$APP"
