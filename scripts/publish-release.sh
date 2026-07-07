#!/usr/bin/env bash
# Publishes a GitHub release with DMG + Sparkle update zip, then updates appcast.xml.
# Usage: scripts/publish-release.sh [version]
# Requires: gh auth login (or GH_TOKEN env var)
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib/version.sh

VERSION="${1:-$(get_version)}"
APP_NAME="小小万年历"
DMG_PATH="$(release_dmg_path "$VERSION")"
ZIP_PATH="dist/${RELEASE_DMG_NAME}-${VERSION}.zip"
TAG="v${VERSION}"
NOTES_FILE="docs/releases/${VERSION}.md"
APPCAST_FILE="appcast/appcast.xml"
NOTES_GENERATED=false

if ! command -v gh >/dev/null 2>&1; then
    echo "error: gh CLI not found. Install with: brew install gh" >&2
    exit 1
fi

if ! gh auth status >/dev/null 2>&1; then
    echo "error: not logged in to GitHub. Run: gh auth login" >&2
    exit 1
fi

if [[ ! -f "$DMG_PATH" ]]; then
    echo "error: $DMG_PATH not found. Run: scripts/build-dmg.sh --version $VERSION" >&2
    exit 1
fi

if [[ ! -f "$NOTES_FILE" ]]; then
    echo "Generating release notes..."
    scripts/generate-release-notes.sh "$VERSION"
    NOTES_GENERATED=true
fi

if ! git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "error: tag $TAG not found. Create with: git tag -a $TAG -m \"Release $VERSION\"" >&2
    exit 1
fi

if [[ ! -f "$ZIP_PATH" ]]; then
    echo "Building update zip..."
    scripts/build-update-zip.sh --version "$VERSION"
fi

echo "Generating appcast..."
scripts/generate-appcast.sh "$VERSION"

if ! git ls-remote --exit-code --tags origin "$TAG" >/dev/null 2>&1; then
    echo "Pushing tag $TAG..."
    git push origin "$TAG"
fi

if gh release view "$TAG" >/dev/null 2>&1; then
    echo "Release $TAG already exists. Uploading assets..."
    gh release upload "$TAG" "$DMG_PATH" "$ZIP_PATH" --clobber
else
    echo "Creating release $TAG..."
    gh release create "$TAG" \
        "$DMG_PATH" \
        "$ZIP_PATH" \
        --title "${APP_NAME} ${VERSION}" \
        --notes-file "$NOTES_FILE"
fi

if git diff --quiet -- "$APPCAST_FILE" && [[ "$NOTES_GENERATED" != true ]]; then
    echo "appcast.xml unchanged"
else
    files_to_commit=()
    if ! git diff --quiet -- "$APPCAST_FILE" 2>/dev/null || [[ -n "$(git status --porcelain -- "$APPCAST_FILE")" ]]; then
        files_to_commit+=("$APPCAST_FILE")
    fi
    if [[ "$NOTES_GENERATED" == true ]]; then
        files_to_commit+=("$NOTES_FILE")
    fi

    if [[ ${#files_to_commit[@]} -gt 0 ]]; then
        echo "Committing ${files_to_commit[*]}..."
        git add "${files_to_commit[@]}"
        git commit -m "Prepare release ${VERSION}"
        git push origin HEAD
    fi
fi

echo "Published: https://github.com/cjhuaxin/tiny-chinese-lunar-calendar/releases/tag/${TAG}"
echo "Appcast: https://raw.githubusercontent.com/cjhuaxin/tiny-chinese-lunar-calendar/main/appcast/appcast.xml"
