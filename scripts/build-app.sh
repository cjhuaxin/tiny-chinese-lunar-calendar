#!/usr/bin/env bash
# Builds 小小万年历.app from the release binary.
# Usage: scripts/build-app.sh [--version X.Y.Z]
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib/version.sh

APP_NAME="小小万年历"
BIN_NAME="tiny-chinese-lunar-calendar"
BUNDLE_ID="com.cjhuaxin.tclc"
GITHUB_REPO="https://github.com/cjhuaxin/tiny-chinese-lunar-calendar"
APPCAST_URL="$(appcast_runtime_feed_url)"
SPARKLE_PUBLIC_KEY_FILE="appcast/sparkle_public_key.txt"
SPARKLE_FRAMEWORK="Sparkle.framework"

REQUESTED_VERSION=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            REQUESTED_VERSION="${2:-}"
            if [[ -z "$REQUESTED_VERSION" ]]; then
                echo "error: --version requires a value" >&2
                exit 1
            fi
            shift 2
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            echo "usage: scripts/build-app.sh [--version X.Y.Z]" >&2
            exit 1
            ;;
    esac
done

VERSION="$(resolve_version "$REQUESTED_VERSION")"
BUILD_NUMBER="${BUILD_NUMBER:-$(semver_to_build_number "$VERSION")}"

if [[ ! -d "$SPARKLE_FRAMEWORK" ]]; then
    echo "error: $SPARKLE_FRAMEWORK not found. Run: scripts/download-sparkle.sh" >&2
    exit 1
fi

if [[ ! -f "$SPARKLE_PUBLIC_KEY_FILE" ]]; then
    echo "error: $SPARKLE_PUBLIC_KEY_FILE not found. Run: ./sparkle-bin/generate_keys" >&2
    exit 1
fi

SPARKLE_PUBLIC_KEY="$(tr -d '[:space:]' < "$SPARKLE_PUBLIC_KEY_FILE")"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}" SPARKLE_FRAMEWORK_DIR="$PWD" cargo build --release

TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
APP_DIR="dist/${APP_NAME}.app"

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources" "$APP_DIR/Contents/Frameworks"

cp "$TARGET_DIR/release/$BIN_NAME" "$APP_DIR/Contents/MacOS/$BIN_NAME"
cp -R "$SPARKLE_FRAMEWORK" "$APP_DIR/Contents/Frameworks/"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP_DIR/Contents/MacOS/$BIN_NAME" 2>/dev/null || true
cp assets/icon/icon.icns "$APP_DIR/Contents/Resources/icon.icns"

cat > "$APP_DIR/Contents/Resources/Credits.html" <<EOF
<html>
<head>
<meta charset="UTF-8">
<style>
body {
    font-family: -apple-system, "PingFang SC", sans-serif;
    font-size: 11px;
    text-align: center;
    color: #666;
    margin: 0;
    padding: 0;
}
a { color: #0066cc; }
</style>
</head>
<body>
<p><a href="${GITHUB_REPO}">GitHub 项目主页</a></p>
</body>
</html>
EOF

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>zh_CN</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleExecutable</key>
    <string>${BIN_NAME}</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${BUILD_NUMBER}</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>SUFeedURL</key>
    <string>${APPCAST_URL}</string>
    <key>SUPublicEDKey</key>
    <string>${SPARKLE_PUBLIC_KEY}</string>
    <key>SUEnableAutomaticChecks</key>
    <true/>
</dict>
</plist>
PLIST

codesign --force --sign - "$APP_DIR/Contents/Frameworks/Sparkle.framework"
codesign --force --deep --sign - "$APP_DIR"

echo "Built $APP_DIR (version $VERSION, build $BUILD_NUMBER)"
