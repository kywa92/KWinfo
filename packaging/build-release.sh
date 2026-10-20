#!/bin/sh
# The release build that the Debian package, the AppImage and the kwinfo-cli tarball are
# made from.
set -eu

cd "$(dirname "$0")/.."
cargo build --release --locked --package kwinfo-app --package kwinfo-cli --package kwinfo-helper
