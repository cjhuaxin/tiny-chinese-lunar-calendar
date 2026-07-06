#!/usr/bin/env bash
# Builds 小小万年历.app from the release binary.
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="小小万年历"
BIN_NAME="tiny-chinese-lunar-calendar"
BUNDLE_ID="com.cjhuaxin.tclc"
VERSION="0.1.1"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}" cargo build --release

TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
APP_DIR="dist/${APP_NAME}.app"

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cp "$TARGET_DIR/release/$BIN_NAME" "$APP_DIR/Contents/MacOS/$BIN_NAME"
cp assets/icon/icon.icns "$APP_DIR/Contents/Resources/icon.icns"

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
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP_DIR"

echo "Built $APP_DIR"
