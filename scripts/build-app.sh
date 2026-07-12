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
BUILD_NUMBER="${BUILD_NUMBER:-$(build_number)}"

if [[ ! -d "$SPARKLE_FRAMEWORK" ]]; then
    echo "error: $SPARKLE_FRAMEWORK not found. Run: scripts/download-sparkle.sh" >&2
    exit 1
fi

if [[ ! -f "$SPARKLE_PUBLIC_KEY_FILE" ]]; then
    echo "error: $SPARKLE_PUBLIC_KEY_FILE not found. Run: ./sparkle-bin/generate_keys" >&2
    exit 1
fi

if [[ -z "${QWEATHER_API_HOST:-}" || -z "${QWEATHER_KID:-}" || -z "${QWEATHER_PROJECT_ID:-}" ]] \
    && [[ ! -f qweather.local.json ]] \
    && [[ ! -f qweather.private.pem ]]; then
    echo "error: QWeather JWT credentials not found." >&2
    echo "  1. openssl genpkey -algorithm ED25519 -out qweather.private.pem" >&2
    echo "  2. openssl pkey -pubout -in qweather.private.pem > qweather.public.pem" >&2
    echo "  3. Upload qweather.public.pem to the QWeather console (JWT credential)" >&2
    echo "  4. Copy qweather.local.example.json to qweather.local.json and fill api_host/kid/project_id" >&2
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

# Credits.rtf, not Credits.html: the About panel imports HTML credits through
# WebKit synchronously on the main thread, which beachballs for 1-2s on first
# open. RTF (with a hyperlink field) renders instantly.
# \u39033?\u30446?\u20027?\u39029? = 项目主页
cat > "$APP_DIR/Contents/Resources/Credits.rtf" <<EOF
{\rtf1\ansi\uc1
{\colortbl;\red102\green102\blue102;\red0\green102\blue204;}
\qc\fs22\cf1
{\field{\*\fldinst{HYPERLINK "${GITHUB_REPO}"}}{\fldrslt\cf2 GitHub \u39033?\u30446?\u20027?\u39029?}}
}
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
    <key>NSLocationWhenInUseUsageDescription</key>
    <string>用于显示您所在城市的天气信息</string>
</dict>
</plist>
PLIST

# Resolve the code signing identity. A real identity (vs ad-hoc "-") is
# required for macOS TCC grants (e.g. location) to survive app updates: the
# permission is tied to the signature's designated requirement, and ad-hoc
# signatures change on every build.
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:-}"
if [[ -z "$CODESIGN_IDENTITY" && -f signing.local.json ]]; then
    CODESIGN_IDENTITY="$(python3 -c 'import json;print(json.load(open("signing.local.json")).get("identity",""))' 2>/dev/null || true)"
fi
if [[ -z "$CODESIGN_IDENTITY" ]]; then
    CODESIGN_IDENTITY="-"
    echo "warning: no signing identity configured (CODESIGN_IDENTITY or signing.local.json)." >&2
    echo "warning: falling back to ad-hoc signing; users will be re-asked for" >&2
    echo "warning: location permission after every update." >&2
fi

# --timestamp embeds a secure timestamp so signatures stay valid after the
# certificate expires (already-installed apps keep working). Not applicable
# to ad-hoc signing.
if [[ "$CODESIGN_IDENTITY" != "-" ]]; then
    codesign --force --timestamp --sign "$CODESIGN_IDENTITY" "$APP_DIR/Contents/Frameworks/Sparkle.framework"
    codesign --force --deep --timestamp --sign "$CODESIGN_IDENTITY" "$APP_DIR"
else
    codesign --force --sign - "$APP_DIR/Contents/Frameworks/Sparkle.framework"
    codesign --force --deep --sign - "$APP_DIR"
fi

if [[ "$CODESIGN_IDENTITY" != "-" ]]; then
    echo "Signed with: $CODESIGN_IDENTITY"
fi
echo "Built $APP_DIR (version $VERSION, build $BUILD_NUMBER)"
