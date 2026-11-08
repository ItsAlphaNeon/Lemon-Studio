#!/usr/bin/env bash
# Copyright (C) 2026 ItsAlphaNeon
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

# Packs a release build of Lemon Studio as an AppImage (the release workflow's Linux download):
#
#   tools/package/appimage.sh <lemonstudio program> <version> <out.AppImage> [nightly]
#
# The AppDir holds the program, its desktop entry (crates/os-app/assets/lemonstudio.desktop, with the version added)
# and its icon. A nightly's is named and looks its own (lemonstudio-nightly, as the program's app id says), so that it
# sits beside Lemon Studio on a desktop. No libraries go in: the program links only the C and C++ runtimes and ALSA, which every desktop
# distribution has, and loads X11, Wayland and JACK itself when they are there. It is built on an older Ubuntu than CI
# uses so that those runtimes are old enough for the distributions in use.
#
# Needs appimagetool: on the PATH, or named by APPIMAGETOOL (the workflow downloads it). The AppImage runtime it adds
# is the static one, which needs no FUSE library on the user's system.
set -euo pipefail

if [ $# -lt 3 ] || [ $# -gt 4 ]; then
    echo "usage: $0 <lemonstudio program> <version> <out.AppImage> [nightly]" >&2
    exit 2
fi
bin=$1
version=$2
out=$3
if [ "${4:-}" = nightly ]; then
    id=lemonstudio-nightly
    name="Lemon Studio Nightly"
else
    id=lemonstudio
    name="Lemon Studio"
fi
root=$(cd "$(dirname "$0")/../.." && pwd)
assets=$root/crates/os-app/assets

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir=$work/LemonStudio.AppDir
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/icons/hicolor/256x256/apps"

install -m755 "$bin" "$appdir/usr/bin/lemonstudio"
install -m644 "$assets/$id.png" "$appdir/usr/share/icons/hicolor/256x256/apps/$id.png"
install -m644 "$assets/$id.png" "$appdir/$id.png"
ln -s "$id.png" "$appdir/.DirIcon"
sed -e "/^\[Desktop Entry\]/a X-AppImage-Version=$version" -e "s/^Name=.*/Name=$name/" -e "s/^Icon=.*/Icon=$id/" \
    -e "s/^StartupWMClass=.*/StartupWMClass=$id/" "$assets/lemonstudio.desktop" >"$appdir/$id.desktop"
cp "$appdir/$id.desktop" "$appdir/usr/share/applications/$id.desktop"

# The AppImage runtime sets APPIMAGE to the AppImage's own path, which is what Lemon Studio's updater replaces.
cat >"$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/lemonstudio" "$@"
APPRUN
chmod 755 "$appdir/AppRun"

ARCH=$(uname -m) "${APPIMAGETOOL:-appimagetool}" --no-appstream "$appdir" "$out"
echo "$out"
