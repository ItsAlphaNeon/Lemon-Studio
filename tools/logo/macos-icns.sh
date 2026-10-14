#!/bin/sh
# Generate the macOS app icon from the Icon Composer document crates/os-app/assets/logo/macOS.icon:
#
#     tools/logo/macos-icns.sh            (on macOS, with Icon Composer in /Applications; run from anywhere)
#
# Writes crates/os-app/assets/lemonstudio.icns (every size an .icns holds, the Default rendition, 8 bits per channel in
# Display P3), which `cargo run -p bundle-app` puts in Lemon Studio.app. The .icns is committed so a build needs no Icon
# Composer.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
ictool="${ICTOOL:-/Applications/Icon Composer.app/Contents/Executables/ictool}"
src="$root/crates/os-app/assets/logo/macOS.icon"
out="$root/crates/os-app/assets/lemonstudio.icns"
set_dir=$(mktemp -d)/lemonstudio.iconset
mkdir -p "$set_dir"

# icon_<points>x<points>[@2x].png: the image at that size and scale.
for pt in 16 32 128 256 512; do
    for scale in 1 2; do
        name="icon_${pt}x${pt}"
        [ "$scale" = 2 ] && name="${name}@2x"
        "$ictool" "$src" --export-image --output-file "$set_dir/$name.png" --platform macOS --rendition Default \
            --width "$pt" --height "$pt" --scale "$scale" > /dev/null
    done
done
# ictool writes 16 bits per channel: four times the file for nothing an icon shows.
cat > "$set_dir/../to8.swift" <<'SWIFT'
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
swift "$set_dir/../to8.swift" "$set_dir"/*.png
iconutil -c icns -o "$out" "$set_dir"
rm -rf "$(dirname "$set_dir")"
echo "$out"
