#!/bin/sh
# Set the workspace version and move every lockfile with it.
#
#     scripts/bump.sh X.Y.Z
#
# The fuzz targets are a workspace of their own, and their lockfile
# pins the greycard crates by version, so a bump of Cargo.toml alone
# leaves it stale and CI's `cargo check --locked` on it refuses.
# This edits the one line, then updates the greycard crates in both
# lockfiles and nothing else. Review and commit the four files
# together: Cargo.toml, Cargo.lock, fuzz/Cargo.lock, and the
# changelog's heading.

set -eu

cd "$(dirname "$0")/.."

version=${1:?usage: scripts/bump.sh X.Y.Z}
case "$version" in
*[!0-9.]*|.*|*.|*..*) echo "bump.sh: $version is not X.Y.Z" >&2; exit 1 ;;
esac

sed -i.bak "s/^version = \"[0-9.]*\"/version = \"$version\"/" Cargo.toml
rm Cargo.toml.bak
cargo update --workspace --offline --quiet
cargo update --workspace --offline --quiet --manifest-path fuzz/Cargo.toml

git --no-pager diff --stat -- Cargo.toml Cargo.lock fuzz/Cargo.lock
