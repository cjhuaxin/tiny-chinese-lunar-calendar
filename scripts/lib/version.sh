#!/usr/bin/env bash
# Shared version helpers. Source from other build scripts.

# ASCII-only DMG basename for GitHub releases (matches 0.1.1 naming).
RELEASE_DMG_NAME="xiaoxiao-wannianli"

release_dmg_path() {
    local version="$1"
    echo "dist/${RELEASE_DMG_NAME}-${version}.dmg"
}

get_version() {
    sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1
}

set_version() {
    local new_version="$1"

    if ! [[ "$new_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "error: invalid version: $new_version (expected semver like 1.2.3)" >&2
        return 1
    fi

    local current
    current="$(get_version)"
    if [[ "$current" == "$new_version" ]]; then
        return 0
    fi

    sed -i '' "s/^version = \".*\"/version = \"${new_version}\"/" Cargo.toml
    echo "Updated Cargo.toml version: $current -> $new_version" >&2
}

# Applies an optional requested version to Cargo.toml, then returns the resolved version.
resolve_version() {
    local requested="${1:-${VERSION:-}}"
    if [[ -n "$requested" ]]; then
        set_version "$requested"
    fi
    cargo pkgid | sed 's/.*@//'
}

# CFBundleVersion / sparkle:version: git commit count plus a fixed offset.
# The offset keeps new builds above the historical semver-derived numbers
# (v0.2.5 shipped as build 205), so Sparkle updates keep working for old
# installs. Never lower this offset.
BUILD_NUMBER_OFFSET=200

build_number() {
    local commits
    commits="$(git rev-list --count HEAD 2>/dev/null || echo 0)"
    echo $(( BUILD_NUMBER_OFFSET + commits ))
}

# ── Cloudflare R2 update distribution (optional) ──
# Configure by copying r2.local.example.json to r2.local.json. When present,
# the Sparkle feed and enclosure URLs point at R2 (fast/reliable from China)
# and publish-release.sh mirrors the artifacts there; GitHub Releases remain
# the fallback copy.
R2_CONFIG_FILE="r2.local.json"

r2_configured() {
    [[ -f "$R2_CONFIG_FILE" ]]
}

r2_get() {
    python3 -c "import json; print(json.load(open('$R2_CONFIG_FILE'))['$1'])"
}

# r2_upload <local-file> <remote-key> [content-type]
r2_upload() {
    local file="$1" key="$2" ctype="${3:-application/octet-stream}"
    AWS_ACCESS_KEY_ID="$(r2_get access_key_id)" \
    AWS_SECRET_ACCESS_KEY="$(r2_get secret_access_key)" \
    AWS_DEFAULT_REGION="auto" \
    aws s3 cp "$file" "s3://$(r2_get bucket)/${key}" \
        --endpoint-url "https://$(r2_get account_id).r2.cloudflarestorage.com" \
        --content-type "$ctype" \
        --no-progress
}

# Returns the repository's default branch (e.g. main).
get_default_branch() {
    local branch
    branch="$(git rev-parse --abbrev-ref origin/HEAD 2>/dev/null | sed 's|^origin/||')"
    echo "${branch:-main}"
}

appcast_feed_url() {
    local repo="${1:-cjhuaxin/tiny-chinese-lunar-calendar}"
    local branch
    branch="$(get_default_branch)"
    # Repo copy used for publishing; may lag behind CDN cache.
    echo "https://cdn.jsdelivr.net/gh/${repo}@${branch}/appcast/appcast.xml"
}

# Stable feed URL baked into the app. Prefers the R2 mirror (reachable from
# China); falls back to the latest GitHub Release asset.
appcast_runtime_feed_url() {
    local repo="${1:-cjhuaxin/tiny-chinese-lunar-calendar}"
    if r2_configured; then
        echo "$(r2_get public_base_url)/appcast.xml"
    else
        echo "https://github.com/${repo}/releases/latest/download/appcast.xml"
    fi
}

# Purge the jsDelivr cache so Sparkle sees the latest appcast immediately after publish.
purge_appcast_cache() {
    local feed_url purge_path
    feed_url="$(appcast_feed_url)"
    purge_path="${feed_url#https://cdn.jsdelivr.net}"

    echo "Purging jsDelivr cache for ${feed_url}..."
    if curl -fsS "https://purge.jsdelivr.net${purge_path}" >/dev/null; then
        echo "jsDelivr cache purged"
    else
        echo "warning: failed to purge jsDelivr cache" >&2
        return 1
    fi
}

# Returns the release tag immediately before v{version}, or empty if none.
previous_release_tag() {
    local version="$1"
    local target_tag="v${version}"
    local prev_tag=""

    while IFS= read -r tag; do
        if [[ "$tag" == "$target_tag" ]]; then
            break
        fi
        prev_tag="$tag"
    done < <(git tag -l 'v*' --sort=version:refname)

    echo "$prev_tag"
}
