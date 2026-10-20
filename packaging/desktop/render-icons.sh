#!/bin/sh
# Renders KWinfo's PNG icons from the SVG sources, into icons/hicolor/<size>x<size>/apps/.
# 16 to 32 px come from icons/src/io.github.kywa92.KWinfo-small.svg (no grid, a bolder
# line), the larger sizes from the scalable icon. Needs rsvg-convert (librsvg2-bin on
# Debian and Ubuntu, librsvg on Homebrew). The PNGs are committed; run this after
# changing either SVG.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
id=io.github.kywa92.KWinfo
large="$here/icons/hicolor/scalable/apps/$id.svg"
small="$here/icons/src/$id-small.svg"

command -v rsvg-convert >/dev/null 2>&1 || {
    echo "render-icons.sh: rsvg-convert is not installed" >&2
    exit 1
}

for size in 16 24 32 48 64 128 256 512; do
    if [ "$size" -le 32 ]; then source=$small; else source=$large; fi
    dir="$here/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dir"
    rsvg-convert --width "$size" --height "$size" --output "$dir/$id.png" "$source"
    echo "wrote ${dir#"$here"/}/$id.png"
done
