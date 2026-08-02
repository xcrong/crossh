#!/usr/bin/env bash
# 打包 crossh.app（macOS）：release 构建 → 组装 .app bundle → ad-hoc 签名 → zip。
#
# 用法:  scripts/package.sh [version]
# 输出:  dist/crossh.app 与 dist/crossh-<version>-macos.zip
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="${1:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)".*/\1/')}"
APP_NAME="crossh"
BUNDLE_ID="io.crossh.app"
DIST="dist"
APP_DIR="$DIST/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

echo "==> cargo build --release"
cargo build --release

echo "==> assembling $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$MACOS" "$RESOURCES"

cp "target/release/$APP_NAME" "$MACOS/$APP_NAME"
cp assets/appicon/AppIcon.icns "$RESOURCES/AppIcon.icns"

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$APP_NAME</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>crossh</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

echo "==> ad-hoc signing (runs locally, no Apple developer account needed)"
codesign --force --deep --sign - "$APP_DIR"

echo "==> zipping"
ZIP="$DIST/$APP_NAME-$VERSION-macos.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP_DIR" "$ZIP"

echo "==> done:"
echo "    $APP_DIR"
echo "    $ZIP"
