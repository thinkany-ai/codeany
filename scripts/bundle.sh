#!/bin/sh
# Build target/codeany.app from the current debug (or `release`) build.
set -e
cd "$(dirname "$0")/.."
PROFILE=${1:-debug}
if [ "$PROFILE" = release ]; then cargo build --release; else cargo build; fi
APP=target/codeany.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$PROFILE/codeany" "$APP/Contents/MacOS/codeany"
cp assets/codeany.icns "$APP/Contents/Resources/"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>CodeAny</string>
  <key>CFBundleDisplayName</key><string>CodeAny</string>
  <key>CFBundleIdentifier</key><string>cc.idoubi.codeany</string>
  <key>CFBundleExecutable</key><string>codeany</string>
  <key>CFBundleIconFile</key><string>codeany</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
PLIST
echo "$APP"
