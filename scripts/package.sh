#!/usr/bin/env bash
# 打包 crossh.app（macOS）：release 构建 → 组装未签名 .app bundle → zip。
#
# 用法:  scripts/package.sh [target] [version]
#         target: aarch64-apple-darwin（默认，arm64 宿主机本机）| x86_64-apple-darwin
# 输出:  dist/crossh.app 与 dist/crossh-<version>-<arch>-macos.zip
set -euo pipefail

cd "$(dirname "$0")/.."

TARGET="${1:-}"
VERSION="${2:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)".*/\1/')}"
APP_NAME="crossh"
BUNDLE_ID="io.crossh.app"
DIST="dist"
APP_DIR="$DIST/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

CARGO_ARGS=("--release")
BIN_DIR="target/release"
ARCH="$(uname -m)"
if [ -n "$TARGET" ]; then
    echo "==> rustup target add $TARGET"
    rustup target add "$TARGET"
    CARGO_ARGS+=("--target" "$TARGET")
    BIN_DIR="target/$TARGET/release"
    case "$TARGET" in
        x86_64-*) ARCH="x86_64" ;;
        aarch64-*) ARCH="aarch64" ;;
    esac
fi

echo "==> cargo build --release${TARGET:+ --target $TARGET}"
cargo build "${CARGO_ARGS[@]}" --bin crossh --bin crossh-git --bin crossh-updater

echo "==> assembling $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$MACOS" "$RESOURCES"
bash scripts/copy-shared-assets.sh "$RESOURCES/crossh-assets"

cp "$BIN_DIR/$APP_NAME" "$MACOS/$APP_NAME"
cp "$BIN_DIR/crossh-git" "$MACOS/crossh-git"
cp "$BIN_DIR/crossh-updater" "$MACOS/crossh-updater"
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
    <key>LSMultipleInstancesProhibited</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

echo "==> signing nested executable"
# A stable ad-hoc signature binds the bundle identifier to the executable.
# Without it, macOS can treat every rebuilt development bundle as a different
# notification client even though Info.plist still declares io.crossh.app.
codesign --force --sign - --identifier "$BUNDLE_ID.git" "$MACOS/crossh-git"
codesign --force --sign - --identifier "$BUNDLE_ID.updater" "$MACOS/crossh-updater"

echo "==> signing app bundle"
codesign --force --sign - --identifier "$BUNDLE_ID" "$APP_DIR"

echo "==> verifying app bundle signature"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

echo "==> zipping"
ZIP="$DIST/$APP_NAME-$VERSION-$ARCH-macos.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP_DIR" "$ZIP"

echo "==> done:"
echo "    $APP_DIR"
echo "    $ZIP"
