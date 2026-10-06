#!/usr/bin/env bash
# Build the Debian package: it puts twaco in /usr/bin. `twaco update` leaves this install alone,
# so install the next .deb to update it.
#
#   packaging/linux/deb.sh VERSION BINARY OUTPUT.deb
set -euo pipefail

version=$1 binary=$2 output=$3
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

# The newest GLIBC_x.y symbol the binary links against is the oldest glibc it runs on.
glibc=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1)

install -d "$root/DEBIAN" "$root/usr/bin" "$root/usr/share/doc/twaco"
install -m 755 "$binary" "$root/usr/bin/twaco"
install -m 644 LICENSE "$root/usr/share/doc/twaco/copyright"
cat > "$root/DEBIAN/control" <<CONTROL
Package: twaco
Version: $version
Architecture: amd64
Maintainer: butteredstardust <https://github.com/butteredstardust/twaco>
Depends: libc6 (>= $glibc)
Section: devel
Priority: optional
Homepage: https://github.com/butteredstardust/twaco
Description: Keep a ThingWorx solution in source control
 Service scripts as files, gates, safe deploys, and an MCP server for AI agents.
CONTROL
dpkg-deb --root-owner-group --build "$root" "$output"
