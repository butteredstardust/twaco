#!/usr/bin/env bash
# Build the macOS installer package: it puts twaco in /usr/local/bin.
#
# WARNING: the package is not signed. Gatekeeper blocks a double-click; open it with
# right-click > Open, or run `sudo installer -pkg <file> -target /`.
#
#   packaging/macos/pkg.sh VERSION BINARY OUTPUT.pkg
set -euo pipefail

version=$1 binary=$2 output=$3
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

install -d "$root/usr/local/bin"
install -m 755 "$binary" "$root/usr/local/bin/twaco"
# Extended attributes ship as ._ files in the payload. The release workflow checks for them.
xattr -cr "$root" 2>/dev/null || true
COPYFILE_DISABLE=1 pkgbuild --root "$root" \
  --identifier io.github.butteredstardust.twaco \
  --version "$version" \
  --install-location / \
  "$output"
