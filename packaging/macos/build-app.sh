#!/bin/sh
set -eu

gpui="$1"
version="${2#v}"
output="$3"
root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$output/Muzik.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$gpui" "$app/Contents/MacOS/muzik-gpui"

icons="$(mktemp -d)"
trap 'rm -rf "$icons"' EXIT
sips -s format png --resampleHeightWidth 512 512 "$root/assets/muzik-logo-v2.png" --out "$icons/icon.png" >/dev/null
sips -s format icns "$icons/icon.png" --out "$app/Contents/Resources/muzik.icns" >/dev/null

cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Muzik</string>
  <key>CFBundleDisplayName</key><string>Muzik</string>
  <key>CFBundleIdentifier</key><string>com.tudorandrei.muzik</string>
  <key>CFBundleExecutable</key><string>muzik-gpui</string>
  <key>CFBundleIconFile</key><string>muzik</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
