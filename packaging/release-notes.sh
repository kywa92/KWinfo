#!/bin/sh
# Prints the notes of the GitHub release for a version: which file to download, then the
# version's section of CHANGELOG.md. The release workflow puts them on the draft release
# and on the summary page of every run.
#
#   packaging/release-notes.sh 0.1.0
set -eu

cd "$(dirname "$0")/.."
version=${1:?usage: $0 VERSION}
version=${version#v}

# The lines after "## [VERSION]" up to the next "## [", without link definitions.
section=$(awk -v heading="## [$version]" '
    index($0, "## [") == 1 {
        if (found) exit
        if (index($0, heading) == 1) { found = 1; next }
    }
    /^\[[^]]+\]: / { next }
    found { print }
    END { if (!found) exit 1 }
' CHANGELOG.md) || {
    echo "release-notes.sh: CHANGELOG.md has no section for $version" >&2
    exit 1
}

cat <<EOF
| File | For |
|---|---|
| \`kwinfo_${version}-1_amd64.deb\` | Ubuntu 22.04 or later, Debian 12 or later: \`sudo apt install ./kwinfo_${version}-1_amd64.deb\` |
| \`KWinfo-x86_64.AppImage\` | other distributions with glibc 2.35 or later: \`chmod +x KWinfo-x86_64.AppImage\`, then run it |
| \`kwinfo-cli-${version}-x86_64-linux-gnu.tar.gz\` | the command line tool and the helper on their own |
| \`SHA256SUMS\` | checking the downloads: \`sha256sum --check --ignore-missing SHA256SUMS\` |

The [installation guide](https://github.com/kywa92/KWinfo/blob/v${version}/docs/install.md)
covers the helper for root-only sensors and the kernel modules a motherboard needs.
$section
EOF
