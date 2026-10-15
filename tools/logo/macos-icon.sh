#!/bin/sh
# Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
# and not any other part of Lemon Studio.

# Generate the macOS app icon from the Icon Composer document crates/os-app/assets/logo/macOS.icon:
#
#     tools/logo/macos-icon.sh            (on macOS, with Xcode and Icon Composer in /Applications; run from anywhere)
#
# Writes, for `cargo run -p bundle-app` to put in Lemon Studio.app:
#   crates/os-app/assets/lemonstudio.car   the compiled asset catalog (Assets.car in the bundle): the icon as macOS 26
#                                          and later draw it, light, dark, tinted and clear (Xcode's actool)
#   crates/os-app/assets/lemonstudio.icns  every size an .icns holds, the Default rendition, 8 bits per channel in
#                                          Display P3 (Icon Composer's ictool): the icon on older macOS
# Both are committed so a build needs neither tool. actool makes an .icns of its own too, but with only two sizes.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
ictool="${ICTOOL:-/Applications/Icon Composer.app/Contents/Executables/ictool}"
# Xcode's tools, whatever `xcode-select` points at (the Command Line Tools have no actool).
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
src="$root/crates/os-app/assets/logo/macOS.icon"
assets="$root/crates/os-app/assets"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The asset catalog. The icon's name in it is its document's: compiled as lemonstudio.icon, it is `lemonstudio`
# (`CFBundleIconName` in the bundle's Info.plist).
cp -R "$src" "$work/lemonstudio.icon"
mkdir "$work/car"
xcrun actool "$work/lemonstudio.icon" --compile "$work/car" --platform macosx --minimum-deployment-target 11.0 \
    --app-icon lemonstudio --output-partial-info-plist "$work/car/partial.plist" --errors --warnings > /dev/null
cp "$work/car/Assets.car" "$assets/lemonstudio.car"

# The .icns: icon_<points>x<points>[@2x].png, the image at that size and scale.
set_dir="$work/lemonstudio.iconset"
mkdir "$set_dir"
for pt in 16 32 128 256 512; do
    for scale in 1 2; do
        name="icon_${pt}x${pt}"
        [ "$scale" = 2 ] && name="${name}@2x"
        "$ictool" "$src" --export-image --output-file "$set_dir/$name.png" --platform macOS --rendition Default \
            --width "$pt" --height "$pt" --scale "$scale" > /dev/null
    done
done
# ictool writes 16 bits per channel: four times the file for nothing an icon shows.
cat > "$work/to8.swift" <<'SWIFT'
import AppKit
for path in CommandLine.arguments.dropFirst() {
    let url = URL(fileURLWithPath: path)
    let src = CGImageSourceCreateWithURL(url as CFURL, nil)!
    let img = CGImageSourceCreateImageAtIndex(src, 0, nil)!
    let space = CGColorSpace(name: CGColorSpace.displayP3)!
    let ctx = CGContext(data: nil, width: img.width, height: img.height, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.draw(img, in: CGRect(x: 0, y: 0, width: img.width, height: img.height))
    let dst = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil)!
    CGImageDestinationAddImage(dst, ctx.makeImage()!, nil)
    CGImageDestinationFinalize(dst)
}
SWIFT
xcrun swift "$work/to8.swift" "$set_dir"/*.png
iconutil -c icns -o "$assets/lemonstudio.icns" "$set_dir"
echo "$assets/lemonstudio.car"
echo "$assets/lemonstudio.icns"
