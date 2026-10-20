#!/bin/sh
# Packs the command line tool for machines without a desktop:
# dist/kwinfo-cli-<version>-<arch>-linux-gnu.tar.gz with kwinfo-cli, kwinfo-helper (for
# `kwinfo-cli helper --session` and `sudo ./kwinfo-helper install`), their man pages and
# the licences.
#
#   packaging/cli-tarball.sh             release build, then the tarball
#   packaging/cli-tarball.sh --no-build  pack what target/release already holds
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
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
version=$(cargo pkgid --package kwinfo-cli | sed 's/.*[#@]//')
name="kwinfo-cli-$version-$(uname -m)-linux-gnu"
work="${CARGO_TARGET_DIR:-$root/target}/cli-tarball"
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || date +%s)}

rm -rf "$work"
mkdir -p "$work/$name" "$root/dist"
cp "$release/kwinfo-cli" "$release/kwinfo-helper" "$work/$name/"
cp docs/man/kwinfo-cli.1 docs/man/kwinfo-helper.8 LICENSE-MIT LICENSE-APACHE "$work/$name/"
cat >"$work/$name/README.md" <<EOF
# kwinfo-cli $version

The command line companion of [KWinfo](https://github.com/kywa92/KWinfo): it lists every
sensor the Linux kernel exposes, captures snapshots and recordings for bug reports, and
bundles diagnostic reports. It needs glibc 2.35 or later and libsensors 3.5 or later
(\`libsensors5\` on Debian and Ubuntu, \`lm_sensors-libs\` on Fedora).

    ./kwinfo-cli list --ids
    man ./kwinfo-cli.1

Root-only sensors (CPU power, core voltages, S.M.A.R.T., GPU load) come from
\`kwinfo-helper\`, which must stay next to kwinfo-cli. \`./kwinfo-cli helper --session\`
starts it through pkexec for one command; \`sudo ./kwinfo-helper install\` sets it up as a
system service that \`kwinfo-cli list\` then uses, and \`sudo
/usr/local/libexec/kwinfo/kwinfo-helper uninstall\` removes it again. See
\`man ./kwinfo-helper.8\` and https://github.com/kywa92/KWinfo/blob/main/docs/install.md.
EOF

tar --create --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
    --directory "$work" "$name" | gzip -9 -n >"$root/dist/$name.tar.gz"
echo "Built $root/dist/$name.tar.gz"
