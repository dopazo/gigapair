#!/usr/bin/env bash
# Builds Gigapair.app and a DMG from the macOS archive that dist produced.
# Runs on macOS only (needs hdiutil, codesign, sips, iconutil).
#
#   scripts/macos-bundle.sh <archive.tar.xz> <version> <output.dmg>
#
# The app is signed ad hoc, not with a Developer ID, and it is not notarized:
# macOS asks for a one-time confirmation on first launch (see README).
set -euo pipefail

archive=${1:?usage: macos-bundle.sh <archive.tar.xz> <version> <output.dmg>}
version=${2:?missing version}
out=${3:?missing output .dmg}

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# 1. Unpack the binaries.
mkdir "$work/archive"
tar -xJf "$archive" -C "$work/archive"
bin_dir=$(dirname "$(find "$work/archive" -name gigapair-app -type f | head -n 1)")
[ -n "$bin_dir" ] || { echo "gigapair-app not found in $archive" >&2; exit 1; }

# 2. The bundle. The service and the CLI sit next to the app executable
# because the app starts `gigapaird` from its own folder.
app="$work/stage/Gigapair.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for exe in gigapair-app gigapaird gigapair gpair; do
  cp "$bin_dir/$exe" "$app/Contents/MacOS/$exe"
  chmod 755 "$app/Contents/MacOS/$exe"
done

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Gigapair</string>
  <key>CFBundleDisplayName</key><string>Gigapair</string>
  <key>CFBundleIdentifier</key><string>dev.gigapair.Gigapair</string>
  <key>CFBundleExecutable</key><string>gigapair-app</string>
  <key>CFBundleIconFile</key><string>Gigapair</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
</dict>
</plist>
PLIST

# 3. The icon: the largest PNG frame of the Windows icon, scaled into an iconset.
python3 - "$root/crates/gigapair/assets/gigapair.ico" "$work/icon.png" <<'PY'
import struct, sys
data = open(sys.argv[1], "rb").read()
count = struct.unpack("<H", data[4:6])[0]
best = None
for i in range(count):
    w = data[6 + 16 * i] or 256
    size, offset = struct.unpack("<II", data[14 + 16 * i:22 + 16 * i])
    if data[offset:offset + 4] == b"\x89PNG" and (best is None or w > best[0]):
        best = (w, data[offset:offset + size])
open(sys.argv[2], "wb").write(best[1])
PY
iconset="$work/Gigapair.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  sips -z $size $size "$work/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) "$work/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Gigapair.icns"

# 4. Ad-hoc signature (Apple silicon refuses to run unsigned code).
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

# 5. The DMG: the app next to a shortcut to Applications.
ln -s /Applications "$work/stage/Applications"
mkdir -p "$(dirname "$out")"
rm -f "$out"
hdiutil create -volname "Gigapair" -srcfolder "$work/stage" -ov -format UDZO "$out" >/dev/null
echo "Built $out"
