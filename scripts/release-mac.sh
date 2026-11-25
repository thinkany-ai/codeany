#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

TEAM_ID=8PL52G56V9
IDENTITY="Developer ID Application: ThinkAny, LLC ($TEAM_ID)"
KEY_ACCOUNT=ai.codeany.desktop
PUBLIC_KEY='zl76lYPXOn1h2cDWYbozH8TNVZBAw/IG5s+RDW3/21Q='
CDN=https://cdn.codeany.ai
BUCKET=codeany-desktop
SPARKLE_VERSION=2.10.1
SPARKLE_SHA=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
SDK="$PWD/target/sparkle-sdk/$SPARKLE_VERSION"
RELEASES="$PWD/target/distribution"
VERSION=$(cargo pkgid --locked | sed 's/.*[#@]//')
DIST="$RELEASES/$VERSION"
DMG="CodeAny_${VERSION}_arm64.dmg"
ZIP="CodeAny_${VERSION}_arm64.zip"

fail() { echo "release: $*" >&2; exit 1; }

fetch_sparkle() {
    local download
    download=$(mktemp -d "$PWD/target/sparkle-sdk/.download.XXXXXX")
    (
        trap 'rm -rf "$download"' EXIT
        curl -fsSL --retry 3 \
            "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz" \
            -o "$download/sparkle.tar.xz"
        echo "$SPARKLE_SHA  $download/sparkle.tar.xz" | shasum -a 256 -c
        mkdir "$download/sdk"
        tar -xf "$download/sparkle.tar.xz" -C "$download/sdk"
        if [ ! -e "$SDK" ]; then mv "$download/sdk" "$SDK"; fi
    )
}

package() {
    [ "$(uname -sm)" = 'Darwin arm64' ] || fail 'packaging requires an Apple Silicon Mac'
    [ ! -e "$DIST" ] || fail "package already exists at $DIST"
    local notary_profile=${CODEANY_NOTARY_PROFILE:-codeany-thinkany}
    /usr/bin/security find-identity -v -p codesigning | grep -F "\"$IDENTITY\"" >/dev/null || fail "missing $IDENTITY"
    mkdir -p "$RELEASES" "$(dirname "$SDK")"
    [ -x "$SDK/bin/generate_appcast" ] || fetch_sparkle
    [ "$("$SDK/bin/generate_keys" --account "$KEY_ACCOUNT" -p)" = "$PUBLIC_KEY" ] || \
        fail "the Keychain must contain the CodeAny Sparkle key for $KEY_ACCOUNT"

    local work app plist framework
    work=$(mktemp -d "$RELEASES/.package.XXXXXX")
    (
        trap 'rm -rf "$work"' EXIT
        app="$work/image/CodeAny.app"
        scripts/bundle.sh release "$app"
        [ "$(/usr/bin/lipo -archs "$app/Contents/MacOS/codeany")" = arm64 ] || fail 'unexpected app architecture'
        plist="$app/Contents/Info.plist"
        /usr/bin/plutil -replace LSMinimumSystemVersion -string '12.0' "$plist"
        /usr/bin/plutil -insert SUFeedURL -string "$CDN/appcast.xml" "$plist"
        /usr/bin/plutil -insert SUPublicEDKey -string "$PUBLIC_KEY" "$plist"
        /usr/bin/plutil -insert SUEnableAutomaticChecks -bool NO "$plist"
        /usr/bin/plutil -insert SUAllowsAutomaticUpdates -bool NO "$plist"
        /usr/bin/plutil -insert SUVerifyUpdateBeforeExtraction -bool YES "$plist"
        /usr/bin/plutil -insert SURequireSignedFeed -bool YES "$plist"

        framework="$app/Contents/Frameworks/Sparkle.framework"
        mkdir -p "$(dirname "$framework")"
        /usr/bin/ditto "$SDK/Sparkle.framework" "$framework"
        # This app is unsandboxed. Sparkle's sandbox-only XPC services aren't used.
        rm -rf "$framework/Versions/B/XPCServices" "$framework/XPCServices"
        sign=(/usr/bin/codesign --force --sign "$IDENTITY" --timestamp --options runtime)
        "${sign[@]}" "$framework/Versions/B/Autoupdate"
        "${sign[@]}" "$framework/Versions/B/Updater.app"
        "${sign[@]}" "$framework/Versions/B"
        "${sign[@]}" "$app"
        /usr/bin/codesign --verify --deep --strict --verbose=2 "$app"

        /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$work/notarize.zip"
        submit_notarization "$work/notarize.zip" "$notary_profile"
        /usr/bin/xcrun stapler staple "$app"
        /usr/bin/xcrun stapler validate "$app"

        mkdir "$work/dist"
        /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$work/dist/$ZIP"
        "$SDK/bin/generate_appcast" --account "$KEY_ACCOUNT" --maximum-deltas 0 \
            --download-url-prefix "$CDN/releases/$VERSION/" --link "$CDN/releases/$VERSION/$DMG" \
            "$work/dist"
        "$SDK/bin/sign_update" --verify --account "$KEY_ACCOUNT" "$work/dist/appcast.xml"

        ln -s /Applications "$work/image/Applications"
        /usr/bin/hdiutil create -quiet -volname CodeAny -srcfolder "$work/image" \
            -format UDZO "$work/dist/$DMG"
        /usr/bin/codesign --sign "$IDENTITY" --timestamp "$work/dist/$DMG"
        submit_notarization "$work/dist/$DMG" "$notary_profile"
        /usr/bin/xcrun stapler staple "$work/dist/$DMG"
        /usr/bin/xcrun stapler validate "$work/dist/$DMG"

        mv "$work/dist" "$DIST"
        echo "Packaged $DIST"
    )
}

submit_notarization() {
    local result status submission
    result=$(/usr/bin/xcrun notarytool submit "$1" --keychain-profile "$2" --wait --output-format json)
    status=$(printf '%s' "$result" | /usr/bin/plutil -extract status raw -)
    submission=$(printf '%s' "$result" | /usr/bin/plutil -extract id raw -)
    echo "Apple notarization $status ($submission)"
    if [ "$status" != Accepted ]; then
        /usr/bin/xcrun notarytool log "$submission" --keychain-profile "$2" >&2
        fail 'Apple declined notarization'
    fi
}

publish() {
    [ -f "$DIST/appcast.xml" ] && [ -f "$DIST/$ZIP" ] && [ -f "$DIST/$DMG" ] || \
        fail "run make release-package first ($DIST)"
    local work status artifact type url signature lock
    work=$(mktemp -d "$RELEASES/.publish.XXXXXX")
    lock="$RELEASES/.publish.lock"
    (
        trap 'rm -rf "$work"' EXIT
        mkdir "$lock" || fail 'another publish is running on this Mac'
        trap 'rmdir "$lock"; rm -rf "$work"' EXIT
        if [ -n "${CODEANY_WRANGLER:-}" ]; then
            wrangler=("$CODEANY_WRANGLER" --cwd "$work")
        else
            wrangler=(npx --yes wrangler@4.154.0 --cwd "$work")
        fi
        # Validate the candidate and reject an older feed before touching R2.
        status=$(curl -sS --retry 3 -H 'Cache-Control: no-cache' -o "$work/live.xml" \
            -w '%{http_code}' "$CDN/appcast.xml")
        case $status in
            200|404) ;;
            *) fail "cannot check the live appcast (HTTP $status)" ;;
        esac
        signature=$(python3 - "$VERSION" "$CDN/releases/$VERSION/$ZIP" "$DIST/$ZIP" \
            "$DIST/appcast.xml" "$work/live.xml" "$status" <<'PY'
import sys
from pathlib import Path
from xml.etree import ElementTree

version, url, archive, candidate, live, status = sys.argv[1:]
sparkle = '{http://www.andymatuschak.org/xml-namespaces/sparkle}'
item = ElementTree.parse(candidate).find('channel/item')
enclosure = item.find('enclosure')
if (item.findtext(sparkle + 'version') != version or enclosure.get('url') != url
        or not enclosure.get(sparkle + 'edSignature')
        or enclosure.get('length') != str(Path(archive).stat().st_size)):
    sys.exit('The appcast does not match the packaged update')
if status == '200':
    versions = [tuple(map(int, value.text.split('.')))
                for value in ElementTree.parse(live).findall('channel/item/' + sparkle + 'version')]
    if not versions or max(versions) > tuple(map(int, version.split('.'))):
        sys.exit('The live appcast is invalid or newer than this package')
    if max(versions) == tuple(map(int, version.split('.'))) and Path(live).read_bytes() != Path(candidate).read_bytes():
        sys.exit('This version already has a different published appcast')
print(enclosure.get(sparkle + 'edSignature'))
PY
        )
        "$SDK/bin/sign_update" --verify --account "$KEY_ACCOUNT" "$DIST/appcast.xml"
        "$SDK/bin/sign_update" --verify --account "$KEY_ACCOUNT" "$DIST/$ZIP" "$signature"
        /usr/bin/codesign --verify --strict -R="anchor apple generic and certificate leaf[subject.OU] = \"$TEAM_ID\"" "$DIST/$DMG"
        /usr/bin/xcrun stapler validate "$DIST/$DMG"
        for artifact in "$ZIP" "$DMG"; do
            case $artifact in
                *.zip) type=application/zip ;;
                *.dmg) type=application/x-apple-diskimage ;;
            esac
            url="$CDN/releases/$VERSION/$artifact"
            status=$(curl -sS --retry 3 -o "$work/artifact" -w '%{http_code}' "$url?release-check=$$")
            case $status in
                404)
                    "${wrangler[@]}" r2 object put "$BUCKET/releases/$VERSION/$artifact" --remote \
                        --file "$DIST/$artifact" --content-type "$type" \
                        --cache-control 'public, max-age=31536000, immutable'
                    curl -fsS --retry 3 -o "$work/artifact" "$url?release-verify=$$"
                    ;;
                200) ;;
                *) fail "cannot check $artifact (HTTP $status)" ;;
            esac
            cmp -s "$DIST/$artifact" "$work/artifact" || fail "CDN has different bytes for $artifact"
        done
        if ! cmp -s "$DIST/appcast.xml" "$work/live.xml"; then
            "${wrangler[@]}" r2 object put "$BUCKET/appcast.xml" --remote \
                --file "$DIST/appcast.xml" --content-type 'application/rss+xml; charset=utf-8' \
                --cache-control 'no-cache, max-age=0, must-revalidate'
        fi
        curl -fsS --retry 3 -H 'Cache-Control: no-cache' -o "$work/live.xml" "$CDN/appcast.xml"
        cmp -s "$DIST/appcast.xml" "$work/live.xml" || fail 'live appcast does not match the release'
        echo "Published $CDN/releases/$VERSION/$DMG"
    )
}

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'Cargo.toml version must use MAJOR.MINOR.PATCH'
case ${1:-} in
    package) package ;;
    publish) publish ;;
    *) fail 'usage: scripts/release-mac.sh {package|publish}' ;;
esac
