#!/usr/bin/env bash
# Flatten the downloaded release-build artifacts into a single directory,
# verify exactly the expected set of non-empty files is present, and write
# SHA256SUMS.txt.
#
# Usage: scripts/ci/collect-release-assets.sh <artifacts-dir> <release-dir>
#
#   <artifacts-dir>  directory holding the downloaded artifacts, one
#                     subdirectory per artifact name -- windows-build/,
#                     linux-build/, extension-build/ -- as produced by
#                     downloading each artifact by name into
#                     "<artifacts-dir>/<name>".
#   <release-dir>     output directory to populate with the flattened,
#                     checksummed release assets. Created if missing.
#
# Why a separate script: build.yml's `release` job only runs on a tag push
# (see its `if:`), so a PR or workflow_dispatch run never exercises the
# collection/verification logic. This script is called from BOTH the real
# `release` job and a `verify-release-assets` job that runs on every
# build.yml run and uploads nothing, so the logic gets tested continuously
# without ever publishing.
#
# Only files matching the allowlisted glob patterns below are ever copied
# into <release-dir> -- an explicit allowlist, not an exclude-list, so a
# stray file from an unrelated artifact (e.g. a linux-distros smoke-test log)
# can never end up in a release, however the artifacts directory was
# populated.

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <artifacts-dir> <release-dir>" >&2
    exit 1
fi

artifacts_dir=$1
release_dir=$2

if [ ! -d "$artifacts_dir" ]; then
    echo "::error::artifacts directory not found: $artifacts_dir" >&2
    exit 1
fi

mkdir -p "$release_dir"

# Expected release filenames, one glob per file -- each must match exactly
# one file. 6 DDMM desktop build outputs (from build-windows and
# build-linux) + 2 browser-extension zips (from build-extension: Chrome and
# Firefox, see extension/scripts/build.mjs) = 8 files total.
patterns=(
    'DDMM-*-windows-x64-setup.exe'
    'DDMM-*-windows-x64-portable.zip'
    'DDMM-*-linux-x86_64.AppImage'
    'DDMM-*-linux-amd64.deb'
    'DDMM-*-linux-x86_64.rpm'
    'DDMM-*-linux-x64.tar.gz'
    'ddmm-extension-chrome-*.zip'
    'ddmm-extension-firefox-*.zip'
)
expected_count=8

# Match each allowlisted pattern against every file under <artifacts-dir>
# (nul-delimited throughout so filenames with spaces are never word-split),
# copy every match into <release-dir>, and track how many files we copied
# per pattern so a missing or duplicated asset is caught below.
shopt -s globstar nullglob dotglob

copied=0
for pattern in "${patterns[@]}"; do
    matches=()
    while IFS= read -r -d '' f; do
        matches+=("$f")
    done < <(find "$artifacts_dir" -type f -name "$pattern" -print0)

    if [ "${#matches[@]}" -eq 0 ]; then
        echo "::error::no file matching '$pattern' found under $artifacts_dir" >&2
        exit 1
    fi
    if [ "${#matches[@]}" -gt 1 ]; then
        echo "::error::expected exactly one file matching '$pattern', found ${#matches[@]}:" >&2
        printf '  %s\n' "${matches[@]}" >&2
        exit 1
    fi

    src=${matches[0]}
    size=$(stat -c%s "$src" 2>/dev/null || stat -f%z "$src")
    if [ "$size" -lt 1 ]; then
        echo "::error::$src matched '$pattern' but is empty (0 bytes)" >&2
        exit 1
    fi

    cp -- "$src" "$release_dir/"
    copied=$((copied + 1))
done

if [ "$copied" -ne "$expected_count" ]; then
    echo "::error::collected $copied release asset(s), expected exactly $expected_count" >&2
    exit 1
fi

# Belt-and-suspenders: verify the release directory itself holds exactly the
# expected file count and no file snuck in some other way, then check every
# file present -- not just the ones we just copied -- is non-empty.
actual_count=$(find "$release_dir" -maxdepth 1 -type f -print | wc -l)
if [ "$actual_count" -ne "$expected_count" ]; then
    echo "::error::$release_dir contains $actual_count file(s), expected exactly $expected_count" >&2
    ls -la "$release_dir" >&2
    exit 1
fi

while IFS= read -r -d '' f; do
    size=$(stat -c%s "$f" 2>/dev/null || stat -f%z "$f")
    if [ "$size" -lt 1 ]; then
        echo "::error::$f is empty (0 bytes)" >&2
        exit 1
    fi
done < <(find "$release_dir" -maxdepth 1 -type f -print0)

(
    cd "$release_dir"
    sha256sum -- * > SHA256SUMS.txt
)

echo "collected $expected_count release asset(s) into $release_dir:"
ls -la "$release_dir"
