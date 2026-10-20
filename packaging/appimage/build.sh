#!/bin/sh
# Builds KWinfo's AppImage, dist/KWinfo-<arch>.AppImage, and its .zsync file for delta
# updates from GitHub releases. The image holds kwinfo, kwinfo-cli and kwinfo-helper in
# usr/bin, the desktop entry, icons and AppStream metadata, and the libraries linuxdeploy
# finds missing from a base system (libsensors). Build it on Ubuntu 22.04: the result
# runs on distributions with glibc 2.35 or later.
#
#   packaging/appimage/build.sh             release build, then the AppImage
#   packaging/appimage/build.sh --no-build  use what target/release already holds
#
# Needs curl, readelf (binutils) and, for the .zsync file, zsyncmake (the zsync
# package). linuxdeploy, its AppImage plugin and the AppImage runtime are downloaded at
# pinned versions and checked against the checksums below; no FUSE is needed to run
# them. UPDATE_INFORMATION overrides the update information embedded in the image.
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

target="${CARGO_TARGET_DIR:-$root/target}"
release="$target/release"
work="$target/appimage"
appdir="$work/AppDir"
tools="$work/tools"
dist="$root/dist"
id=io.github.kywa92.KWinfo

arch=$(uname -m)
linuxdeploy_tag=1-alpha-20251107-1
plugin_tag=1-alpha-20250213-1
runtime_tag=20251108
case "$arch" in
    x86_64)
        linuxdeploy_sha256=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
        plugin_sha256=992d502a248e14ab185448ddf6f6e7d25558cb84d4623c354c3af350c25fccb3
        runtime_sha256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
        ;;
    aarch64)
        linuxdeploy_sha256=620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff
        plugin_sha256=83c292149274965a865dcd44c135cfca8ba28c6b7de3eb628d4b8b5f248af17c
        runtime_sha256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
        ;;
    *)
        echo "build.sh: no AppImage tools for $arch" >&2
        exit 1
        ;;
esac

# fetch URL FILE SHA256: downloads FILE into the tools directory unless a copy with the
# right checksum is already there.
fetch() {
    if [ -f "$tools/$2" ] && echo "$3  $tools/$2" | sha256sum --check --status; then
        return
    fi
    curl --fail --location --silent --show-error --retry 3 --output "$tools/$2.part" "$1"
    if ! echo "$3  $tools/$2.part" | sha256sum --check --status; then
        echo "build.sh: $2 from $1 does not have the expected checksum" >&2
        exit 1
    fi
    chmod +x "$tools/$2.part"
    mv "$tools/$2.part" "$tools/$2"
}

# KWinfo copies kwinfo-helper out of the image to run it as root (to $XDG_RUNTIME_DIR for
# a session, to /usr/local/libexec/kwinfo for `kwinfo-helper install`), away from the
# libraries the image bundles. It must need nothing but glibc.
for lib in $(readelf --dynamic "$release/kwinfo-helper" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p'); do
    case "$lib" in
        libc.so.* | libm.so.* | libgcc_s.so.* | ld-linux*.so.* | libdl.so.* | libpthread.so.* | librt.so.*) ;;
        *)
            echo "build.sh: kwinfo-helper needs $lib, which a copy outside the AppImage would not find" >&2
            exit 1
            ;;
    esac
done

mkdir -p "$tools" "$dist"
fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/$linuxdeploy_tag/linuxdeploy-$arch.AppImage" \
    "linuxdeploy-$arch.AppImage" "$linuxdeploy_sha256"
fetch "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/$plugin_tag/linuxdeploy-plugin-appimage-$arch.AppImage" \
    "linuxdeploy-plugin-appimage-$arch.AppImage" "$plugin_sha256"
fetch "https://github.com/AppImage/type2-runtime/releases/download/$runtime_tag/runtime-$arch" \
    "runtime-$arch" "$runtime_sha256"

# What linuxdeploy does not put there itself: the icons, the AppStream metadata and the
# licences. linuxdeploy adds the binaries, the desktop entry and AppRun. Only the PNG
# icons: with the SVG there, .DirIcon would be an SVG, which AppImage tools read badly.
# The metadata goes under its older name, the only one this appimagetool looks for.
rm -rf "$appdir"
mkdir -p "$appdir/usr/share/icons/hicolor" "$appdir/usr/share/metainfo" \
    "$appdir/usr/share/doc/kwinfo"
for size in "$root"/packaging/desktop/icons/hicolor/*x*; do
    cp -R "$size" "$appdir/usr/share/icons/hicolor/"
done
cp "$root/packaging/desktop/$id.metainfo.xml" "$appdir/usr/share/metainfo/$id.appdata.xml"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/README.md" "$root/docs/install.md" \
    "$appdir/usr/share/doc/kwinfo/"

image="KWinfo-$arch.AppImage"
rm -f "$dist/$image" "$dist/$image.zsync"
if ! command -v zsyncmake >/dev/null 2>&1; then
    echo "build.sh: zsyncmake is not installed, so there will be no $image.zsync" >&2
fi

# linuxdeploy and its plugin are AppImages themselves; this runs them without FUSE.
export APPIMAGE_EXTRACT_AND_RUN=1

# First the AppDir: the binaries, the libraries they need, the desktop entry and AppRun.
"$tools/linuxdeploy-$arch.AppImage" \
    --appdir "$appdir" \
    --executable "$release/kwinfo" \
    --executable "$release/kwinfo-cli" \
    --executable "$release/kwinfo-helper" \
    --desktop-file "$root/packaging/desktop/$id.desktop" \
    --icon-file "$root/packaging/desktop/icons/hicolor/256x256/apps/$id.png" \
    --custom-apprun "$root/packaging/appimage/AppRun"

# linuxdeploy links the AppDir's own icon to whichever size it finds first; file managers
# and AppImage tools show that one (as .DirIcon), so it is the 256 px icon.
ln -sf "usr/share/icons/hicolor/256x256/apps/$id.png" "$appdir/$id.png"
rm -f "$appdir/.DirIcon"

# appimagetool would check the AppStream metadata with network access, and the URLs in
# it only exist once the commit is on GitHub; check it offline here instead.
if command -v appstreamcli >/dev/null 2>&1; then
    appstreamcli validate-tree --no-net "$appdir"
fi

# Then the image, through the plugin, which hands these to appimagetool.
export LDAI_OUTPUT="$image"
export LDAI_RUNTIME_FILE="$tools/runtime-$arch"
export LDAI_UPDATE_INFORMATION="${UPDATE_INFORMATION:-gh-releases-zsync|kywa92|KWinfo|latest|$image.zsync}"
export LDAI_NO_APPSTREAM=1
cd "$dist"
"$tools/linuxdeploy-plugin-appimage-$arch.AppImage" --appdir "$appdir"

echo "Built $dist/$image"
