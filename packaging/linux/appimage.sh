#!/usr/bin/env bash
# Build the AppImage: one file that runs on most x86_64 Linux systems. `twaco update` leaves an
# AppImage alone, so download the next one to update it.
#
# WARNING: downloads appimagetool. Its checksum is pinned below, so a changed download fails.
#
#   packaging/linux/appimage.sh BINARY OUTPUT.AppImage
set -euo pipefail

binary=$1 output=$2
tool_url=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
tool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl -fsSL -o "$work/appimagetool" "$tool_url"
echo "$tool_sha256  $work/appimagetool" | sha256sum --check --quiet
chmod +x "$work/appimagetool"

app="$work/twaco.AppDir"
install -d "$app/usr/bin"
install -m 755 "$binary" "$app/usr/bin/twaco"
install -m 644 "$here/../../assets/icon/twaco-512.png" "$app/twaco.png"
cat > "$app/twaco.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=twaco
Exec=twaco
Icon=twaco
Terminal=true
Categories=Development;
DESKTOP
cat > "$app/AppRun" <<'APPRUN'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/twaco" "$@"
APPRUN
chmod 755 "$app/AppRun"

# The runners have no FUSE, so appimagetool runs from its extracted files.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$work/appimagetool" --no-appstream "$app" "$output"
