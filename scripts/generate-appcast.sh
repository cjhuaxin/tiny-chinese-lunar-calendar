#!/usr/bin/env bash
# Generates or updates appcast/appcast.xml for Sparkle auto-updates.
# Usage: scripts/generate-appcast.sh [version]
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib/version.sh

VERSION="${1:-$(get_version)}"
TAG="v${VERSION}"
# sparkle:version must match the CFBundleVersion baked into the shipped app;
# read it from the built bundle when available (the commit count may have
# moved on since the app was built).
BUILT_PLIST="dist/小小万年历.app/Contents/Info.plist"
if [[ -f "$BUILT_PLIST" ]]; then
    BUILD_NUMBER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$BUILT_PLIST")"
else
    BUILD_NUMBER="$(build_number)"
fi
ZIP_NAME="${RELEASE_DMG_NAME}-${VERSION}.zip"
ZIP_PATH="dist/${ZIP_NAME}"
APPCAST_FILE="appcast/appcast.xml"
PRIVATE_KEY_FILE="sparkle_private_key.txt"
GITHUB_REPO="cjhuaxin/tiny-chinese-lunar-calendar"
RELEASE_URL="https://github.com/${GITHUB_REPO}/releases/tag/${TAG}"
DOWNLOAD_URL="https://github.com/${GITHUB_REPO}/releases/download/${TAG}/${ZIP_NAME}"
NOTES_FILE="docs/releases/${VERSION}.md"

if [[ ! -x "sparkle-bin/sign_update" ]]; then
    echo "error: sparkle-bin/sign_update not found. Run: scripts/download-sparkle.sh" >&2
    exit 1
fi

if [[ ! -f "$ZIP_PATH" ]]; then
    echo "error: $ZIP_PATH not found. Run: scripts/build-update-zip.sh --version $VERSION" >&2
    exit 1
fi

if [[ ! -f "$PRIVATE_KEY_FILE" ]]; then
    echo "error: $PRIVATE_KEY_FILE not found. Run: ./sparkle-bin/generate_keys -x sparkle_private_key.txt" >&2
    exit 1
fi

SIGNATURE_LINE="$(./sparkle-bin/sign_update "$ZIP_PATH" -f "$PRIVATE_KEY_FILE")"
ED_SIGNATURE="$(echo "$SIGNATURE_LINE" | sed -n 's/.*sparkle:edSignature="\([^"]*\)".*/\1/p')"
FILE_LENGTH="$(echo "$SIGNATURE_LINE" | sed -n 's/.*length="\([^"]*\)".*/\1/p')"

if [[ -z "$ED_SIGNATURE" || -z "$FILE_LENGTH" ]]; then
    echo "error: failed to parse sign_update output: $SIGNATURE_LINE" >&2
    exit 1
fi

PUB_DATE="$(date -u '+%a, %d %b %Y %H:%M:%S +0000')"

# Embed the release notes as HTML in <description> so Sparkle shows a clean
# notes view instead of loading the full GitHub release page.
if [[ -f "$NOTES_FILE" ]]; then
    RELEASE_NOTES_HTML="$(python3 scripts/lib/md2html.py < "$NOTES_FILE")"
    RELEASE_NOTES_BLOCK="<description><![CDATA[${RELEASE_NOTES_HTML}]]></description>"
else
    echo "warning: $NOTES_FILE not found; falling back to release page link" >&2
    RELEASE_NOTES_BLOCK="<sparkle:releaseNotesLink>${RELEASE_URL}</sparkle:releaseNotesLink>"
fi

mkdir -p appcast
cat > "$APPCAST_FILE" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:dc="http://purl.org/dc/elements/1.1/">
    <channel>
        <title>小小万年历</title>
        <link>${RELEASE_URL}</link>
        <description>Most recent updates to 小小万年历</description>
        <language>zh-cn</language>
        <item>
            <title>Version ${VERSION}</title>
            <link>${RELEASE_URL}</link>
            <sparkle:version>${BUILD_NUMBER}</sparkle:version>
            <sparkle:shortVersionString>${VERSION}</sparkle:shortVersionString>
            ${RELEASE_NOTES_BLOCK}
            <pubDate>${PUB_DATE}</pubDate>
            <enclosure
                url="${DOWNLOAD_URL}"
                sparkle:edSignature="${ED_SIGNATURE}"
                length="${FILE_LENGTH}"
                type="application/octet-stream" />
        </item>
    </channel>
</rss>
EOF

echo "Generated $APPCAST_FILE"
echo "  version: ${VERSION} (build ${BUILD_NUMBER})"
echo "  enclosure: ${DOWNLOAD_URL}"
