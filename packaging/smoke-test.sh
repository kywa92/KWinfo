#!/bin/sh
# Installs and starts a KWinfo package on a fresh system. The release workflow runs it in
# containers of the distributions KWinfo supports:
#
#   packaging/smoke-test.sh deb dist/kwinfo_0.1.0-1_amd64.deb
#   packaging/smoke-test.sh appimage dist/KWinfo-x86_64.AppImage
#
# Run it as root on a throwaway system: it installs packages. The deb test installs the
# package with apt (which must resolve every dependency), checks the helper's files and
# systemd unit and removes the package again. Both tests run the binaries and start the
# window on a virtual X display with Mesa's software renderer; it must still be running
# after 15 seconds. With SCREENSHOT=file.png the display is saved there after 10 seconds.
set -eu

kind=${1:?usage: $0 deb|appimage FILE}
file=$(readlink -f "${2:?usage: $0 deb|appimage FILE}")
log=$(mktemp)

fail() {
    echo "smoke-test.sh: $*" >&2
    exit 1
}

# A virtual X server and software rendering; for the AppImage also the libraries every
# desktop has, which the image leaves to the system (the .deb depends on them instead).
if command -v apt-get >/dev/null 2>&1; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    packages="xvfb xauth libgl1-mesa-dri mesa-vulkan-drivers"
    if [ "$kind" = deb ]; then
        packages="$packages systemd"
    else
        packages="$packages libegl1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 \
            libwayland-cursor0 libwayland-egl1 libx11-6 libx11-xcb1 libxcursor1 libxi6"
    fi
    if [ -n "${SCREENSHOT-}" ]; then
        packages="$packages x11-apps netpbm"
    fi
    # shellcheck disable=SC2086 # a list of package names
    apt-get install -y -qq --no-install-recommends $packages >/dev/null
elif command -v dnf >/dev/null 2>&1; then
    [ "$kind" = appimage ] || fail "a .deb needs a Debian or Ubuntu system"
    packages="xorg-x11-server-Xvfb xorg-x11-xauth which mesa-dri-drivers mesa-vulkan-drivers \
        mesa-libEGL libxkbcommon libxkbcommon-x11 libwayland-client libwayland-cursor \
        libwayland-egl libX11 libX11-xcb libXcursor libXi"
    if [ -n "${SCREENSHOT-}" ]; then
        packages="$packages xwd netpbm-progs"
    fi
    # shellcheck disable=SC2086 # a list of package names
    dnf install -y -q $packages >/dev/null
else
    fail "no apt-get or dnf to install the test's dependencies with"
fi

# window COMMAND...: starts the window on a virtual display; it must outlive 15 seconds.
window() {
    status=0
    # shellcheck disable=SC2016 # expanded by the inner shell
    xvfb-run --auto-servernum --server-args="-screen 0 1280x800x24" sh -c '
        if [ -n "${SCREENSHOT-}" ]; then
            (sleep 10 && xwd -root -silent | xwdtopnm 2>/dev/null | pnmtopng >"$SCREENSHOT") &
        fi
        timeout 15 "$@"' sh "$@" >"$log" 2>&1 || status=$?
    if [ "$status" -ne 124 ] || grep -q "panicked" "$log"; then
        cat "$log"
        fail "the window ended with status $status instead of running until stopped"
    fi
    echo "ok: the window ran for 15 seconds ($*)"
}

case "$kind" in
    deb)
        apt-get install -y -qq "$file" >/dev/null
        echo "ok: apt installed $(dpkg-query -W -f '${Package} ${Version}' kwinfo)"
        kwinfo --version
        kwinfo-cli --version
        /usr/libexec/kwinfo/kwinfo-helper --version
        kwinfo-cli list --mock >/dev/null
        echo "ok: kwinfo-cli lists the mock sensors"
        grep -q "^ExecStart=/usr/libexec/kwinfo/kwinfo-helper serve$" \
            /usr/lib/systemd/system/kwinfo-helper.service ||
            fail "the systemd unit does not start the packaged helper"
        grep -q "^Exec=/usr/libexec/kwinfo/kwinfo-helper serve$" \
            /usr/share/dbus-1/system-services/io.github.kywa92.KWinfo.Helper.service ||
            fail "the D-Bus activation file does not name the packaged helper"
        systemd-analyze verify /usr/lib/systemd/system/kwinfo-helper.service
        echo "ok: systemd accepts the helper's unit"
        # dpkg -L rather than the file system: Docker's Ubuntu images skip man pages and
        # documentation when installing.
        dpkg -L kwinfo >"$log"
        for path in /usr/share/dbus-1/system.d/io.github.kywa92.KWinfo.Helper.conf \
            /usr/share/polkit-1/actions/io.github.kywa92.KWinfo.policy \
            /usr/lib/modules-load.d/kwinfo.conf \
            /usr/share/applications/io.github.kywa92.KWinfo.desktop \
            /usr/share/metainfo/io.github.kywa92.KWinfo.metainfo.xml \
            /usr/share/icons/hicolor/scalable/apps/io.github.kywa92.KWinfo.svg \
            /usr/share/man/man1/kwinfo.1.gz /usr/share/man/man1/kwinfo-cli.1.gz \
            /usr/share/man/man8/kwinfo-helper.8.gz /usr/share/doc/kwinfo/copyright; do
            grep -qx "$path" "$log" || fail "the package has no $path"
        done
        echo "ok: the D-Bus, polkit, module, desktop, icon, man and copyright files are there"
        window kwinfo --mock
        apt-get remove -y -qq kwinfo >/dev/null
        [ ! -e /usr/libexec/kwinfo/kwinfo-helper ] || fail "the helper is still there after removal"
        apt-get purge -y -qq kwinfo >/dev/null
        echo "ok: removed and purged"
        ;;
    appimage)
        chmod +x "$file"
        # The runtime's own options, which need no FUSE. With APPIMAGE_EXTRACT_AND_RUN set,
        # the runtime would pass them on to kwinfo.
        info=$("$file" --appimage-updateinformation)
        case "$info" in
            "gh-releases-zsync|kywa92|KWinfo|latest|KWinfo-"*.AppImage.zsync) ;;
            *) fail "unexpected update information: $info" ;;
        esac
        echo "ok: update information $info"
        # KWinfo runs the helper as root from a copy outside the image, so such a copy must
        # not need the libraries the image bundles.
        dir=$(mktemp -d)
        (cd "$dir" && "$file" --appimage-extract usr/bin/kwinfo-helper >/dev/null)
        cp "$dir/squashfs-root/usr/bin/kwinfo-helper" "$dir/kwinfo-helper"
        rm -rf "$dir/squashfs-root"
        "$dir/kwinfo-helper" --version
        echo "ok: a copy of the helper runs outside the image"
        # No FUSE in containers: the runtime unpacks the image to a temporary directory.
        export APPIMAGE_EXTRACT_AND_RUN=1
        "$file" --version
        "$file" kwinfo-cli --version
        "$file" kwinfo-cli list --mock >/dev/null
        "$file" kwinfo-helper --version
        echo "ok: the AppImage runs kwinfo-cli and kwinfo-helper"
        window "$file" --mock
        ;;
    *) fail "unknown kind $kind; use deb or appimage" ;;
esac
