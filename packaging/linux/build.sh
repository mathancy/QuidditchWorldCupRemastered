#!/usr/bin/env bash
# The Linux release files from a stage (packaging/stage.py):
#
#   packaging/linux/build.sh VERSION STAGE OUT
#
# writes OUT/QWCRemastered-VERSION-x86_64.AppImage (the name the launcher's updater looks for) and
# OUT/QWCRemastered-VERSION-x86_64-linux.tar.gz (the same files in a folder, to unpack and run qwc-launcher).
# The AppImage is made by appimagetool ($APPIMAGETOOL, else on the PATH). The programs need the system's Vulkan (or
# OpenGL) driver, ALSA and udev, which desktops have; nothing of the game is in either file.
set -euo pipefail
version="$1"
stage="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
icon="$root/crates/qwc-style/assets/qwc_logo.png"
name="QWCRemastered-$version"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# the tarball: the stage in a folder of the version's name, with a desktop entry and the icon
mkdir -p "$work/$name"
cp -r "$stage/." "$work/$name/"
cp "$here/qwc-remastered.desktop" "$work/$name/"
cp "$icon" "$work/$name/qwc-remastered.png"
chmod 755 "$work/$name/qwc-launcher" "$work/$name/qwc-game" "$work/$name/qwc-studio"
tar -C "$work" -czf "$out/$name-x86_64-linux.tar.gz" "$name"

# the AppImage: the stage under usr/lib/qwc-remastered, started by AppRun
app="$work/AppDir"
mkdir -p "$app/usr/lib/qwc-remastered" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/512x512/apps"
cp -r "$work/$name/." "$app/usr/lib/qwc-remastered/"
rm "$app/usr/lib/qwc-remastered/qwc-remastered.desktop" "$app/usr/lib/qwc-remastered/qwc-remastered.png"
install -m 755 "$here/AppRun" "$app/AppRun"
cp "$here/qwc-remastered.desktop" "$app/qwc-remastered.desktop"
cp "$here/qwc-remastered.desktop" "$app/usr/share/applications/"
cp "$icon" "$app/qwc-remastered.png"
cp "$icon" "$app/usr/share/icons/hicolor/512x512/apps/qwc-remastered.png"
ARCH=x86_64 "${APPIMAGETOOL:-appimagetool}" --no-appstream "$app" "$out/$name-x86_64.AppImage"
echo "wrote $out/$name-x86_64.AppImage and $out/$name-x86_64-linux.tar.gz"
