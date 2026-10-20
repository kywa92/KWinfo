#!/bin/sh
# Checks, before a release tag vX.Y.Z publishes anything, that everything names the same
# version: the workspace version in Cargo.toml, a dated "## [X.Y.Z] - YYYY-MM-DD" section
# in CHANGELOG.md, and the AppStream release entry, which must carry that date and no
# longer say type="development". The release workflow runs it on tags.
#
#   packaging/check-release.sh v0.1.0
set -eu

cd "$(dirname "$0")/.."
tag=${1:?usage: $0 vX.Y.Z}
version=${tag#v}
metainfo=packaging/desktop/io.github.kywa92.KWinfo.metainfo.xml

fail() {
    echo "check-release.sh: $*" >&2
    exit 1
}

cargo_version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)
[ "$cargo_version" = "$version" ] ||
    fail "the tag is $tag, but the workspace version in Cargo.toml is $cargo_version"

date=$(sed -n "s/^## \[$version\] - \([0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\}\)\$/\1/p" CHANGELOG.md)
[ -n "$date" ] || fail "CHANGELOG.md has no dated section \"## [$version] - YYYY-MM-DD\""

release=$(grep -o "<release version=\"$version\"[^>]*>" "$metainfo") ||
    fail "$metainfo has no <release version=\"$version\"> entry"
case "$release" in
    *'type="development"'*) fail "$metainfo still marks $version as a development release" ;;
esac
case "$release" in
    *"date=\"$date\""*) ;;
    *) fail "the date of $version in $metainfo is not $date, the date in CHANGELOG.md" ;;
esac

echo "$tag: Cargo.toml, CHANGELOG.md ($date) and the AppStream metadata agree"
