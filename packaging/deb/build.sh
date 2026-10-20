#!/bin/sh
# Builds KWinfo's Debian package, dist/kwinfo_<version>-1_<arch>.deb: kwinfo and
# kwinfo-cli, the helper in /usr/libexec/kwinfo with its systemd unit, D-Bus files, polkit
# action and module list, the desktop entry, icons, AppStream metadata, man pages and
# documentation. Needs Debian or Ubuntu with the build dependencies from README.md and
# cargo-deb (`cargo install cargo-deb`). A package built on Ubuntu 22.04 installs on
# Ubuntu 22.04 and later and on Debian 12 and later.
#
#   packaging/deb/build.sh             release build, then the package
#   packaging/deb/build.sh --no-build  package what target/release already holds
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

case "${1-}" in
    "") "$root/packaging/build-release.sh" ;;
    --no-build) ;;
    *)
        echo "usage: $0 [--no-build]" >&2
        exit 2
        ;;
esac

release="${CARGO_TARGET_DIR:-$root/target}/release"
stage="$release/kwinfo-deb"
version=$(cargo pkgid --package kwinfo-app | sed 's/.*[#@]//')
maintainer=$(sed -n 's/^authors = \["\(.*\)"\]$/\1/p' Cargo.toml)
# The time of the last commit, so that building the same commit twice gives the same
# package (cargo-deb reads it too).
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || date +%s)}
export SOURCE_DATE_EPOCH

rm -rf "$stage"
mkdir -p "$stage"

# The helper's systemd unit, D-Bus files, polkit action and module list, rendered by the
# helper itself for its packaged path.
"$release/kwinfo-helper" install --root "$stage/root" \
    --helper-path /usr/libexec/kwinfo/kwinfo-helper --no-copy >/dev/null

# Debian wants a changelog in its own format; the real one is CHANGELOG.md.
cat >"$stage/changelog.Debian" <<EOF
kwinfo ($version-1) unstable; urgency=medium

  * KWinfo $version. The changes are listed in CHANGELOG.md and at
    https://github.com/kywa92/KWinfo/releases

 -- $maintainer  $(date -u -R -d "@$SOURCE_DATE_EPOCH")
EOF
gzip -9 -n -f "$stage/changelog.Debian"

mkdir -p "$root/dist"
cargo deb --package kwinfo-app --no-build --locked --deb-revision 1 --output "$root/dist/"

echo "Built $(ls "$root/dist/kwinfo_${version}-1_"*.deb)"
