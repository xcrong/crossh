#!/usr/bin/env bash
# 打包 Linux 产物：tar.gz 二进制包 + AppImage（AppDir 组装 + linuxdeploy）+ 配套 install.sh。
#
# 用法:  scripts/package-linux.sh [target] [version]
#         target: x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu（默认按宿主 uname -m 推导）
# 依赖:  rustup、curl、tar、rsvg-convert（Ubuntu 包: librsvg2-bin）
# 输出:  dist/crossh-<version>-linux-<arch>.tar.gz、.AppImage、install.sh 与 -install.tar.gz（含 AppImage+install.sh）
set -euo pipefail

cd "$(dirname "$0")/.."
DIST="$(pwd)/dist"

TARGET="${1:-}"
if [ -z "$TARGET" ]; then
    case "$(uname -m)" in
        x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
        *) echo "unsupported host arch: $(uname -m)" >&2; exit 1 ;;
    esac
fi
VERSION="${2:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)".*/\1/')}"
APP_NAME="crossh"
APP_ID="me.xcrong.crossh"
case "$TARGET" in
    x86_64-*) ARCH="x86_64" ;;
    aarch64-*) ARCH="aarch64" ;;
    *) echo "unsupported target: $TARGET" >&2; exit 1 ;;
esac
STAGE="$DIST/package-linux-$ARCH"
APPDIR="$DIST/$APP_ID.AppDir"
APP_LIB="$APPDIR/usr/lib"
BIN="target/$TARGET/release/$APP_NAME"
GIT_BIN="target/$TARGET/release/crossh-git"
NOTE_BIN="target/$TARGET/release/crossh-note"
UPDATER_BIN="target/$TARGET/release/crossh-updater"

echo "==> rustup target add $TARGET"
rustup target add "$TARGET"

echo "==> cargo build --release --target $TARGET"
cargo build --release --target "$TARGET" --bin crossh --bin crossh-git --bin crossh-note --bin crossh-updater

mkdir -p "$DIST"

# --- tar.gz -----------------------------------------------------------------
echo "==> tar.gz"
rm -rf "$STAGE"
mkdir -p "$STAGE/$APP_NAME-$VERSION-linux-$ARCH"
cp "$BIN" "$GIT_BIN" "$NOTE_BIN" "$UPDATER_BIN" README.md LICENSE "$STAGE/$APP_NAME-$VERSION-linux-$ARCH/"
bash scripts/copy-shared-assets.sh "$STAGE/$APP_NAME-$VERSION-linux-$ARCH/resources/crossh-assets"
TARBALL="$DIST/$APP_NAME-$VERSION-linux-$ARCH.tar.gz"
rm -f "$TARBALL"
tar -C "$STAGE" -czf "$TARBALL" "$APP_NAME-$VERSION-linux-$ARCH"

# --- AppImage ---------------------------------------------------------------
echo "==> assembling AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APP_LIB" \
    "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/512x512/apps"
bash scripts/copy-shared-assets.sh "$APPDIR/usr/bin/resources/crossh-assets"

cp "$BIN" "$APPDIR/usr/bin/$APP_NAME"
cp "$GIT_BIN" "$APPDIR/usr/bin/crossh-git"
cp "$NOTE_BIN" "$APPDIR/usr/bin/crossh-note"
cp "$UPDATER_BIN" "$APPDIR/usr/bin/crossh-updater"
cp README.md LICENSE "$APPDIR/"

cat > "$APPDIR/usr/share/applications/me.xcrong.crossh.desktop" <<EOF
[Desktop Entry]
Name=crossh
GenericName=Terminal Workspace
Comment=Local-first terminal workspace (GPUI)
Exec=crossh
Icon=me.xcrong.crossh
Terminal=false
Type=Application
Categories=Development;
StartupWMClass=me.xcrong.crossh
Keywords=terminal;workspace;project;git;notes;
EOF
ln -s "usr/share/applications/me.xcrong.crossh.desktop" "$APPDIR/me.xcrong.crossh.desktop"

rsvg-convert -w 512 -h 512 assets/appicon/icon-master.svg \
    -o "$APPDIR/usr/share/icons/hicolor/512x512/apps/me.xcrong.crossh.png"
ln -s "usr/share/icons/hicolor/512x512/apps/me.xcrong.crossh.png" "$APPDIR/me.xcrong.crossh.png"
ln -sf "usr/share/icons/hicolor/512x512/apps/me.xcrong.crossh.png" "$APPDIR/.DirIcon"


# --- AppImage（linuxdeploy） --------------------------------------------------
# 依赖组装交由上游 linuxdeploy：内置排除 libc/GL/EGL/Vulkan 等宿主库，
# 另用 --exclude-library 排除显示栈（wayland/xkbcommon/xcb/X），全部走本机。
# 背景：手写 collect_libs 两次翻车（0.31.0 显示栈冲突、0.31.1 丢 continue 把
# libc 打进包），此后不再手写打包规则，见 docs/engineering-notes/appimage-bundled-libs-gpu.md。
echo "==> linuxdeploy"
LINUXDEPLOY="$DIST/linuxdeploy-$ARCH.AppImage"
if [ ! -x "$LINUXDEPLOY" ]; then
    curl -fL -o "$LINUXDEPLOY" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$ARCH.AppImage"
    chmod +x "$LINUXDEPLOY"
fi
LINUXDEPLOY_PLUGIN="$DIST/linuxdeploy-plugin-appimage-$ARCH.AppImage"
if [ ! -x "$LINUXDEPLOY_PLUGIN" ]; then
    curl -fL -o "$LINUXDEPLOY_PLUGIN" \
        "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-$ARCH.AppImage"
    chmod +x "$LINUXDEPLOY_PLUGIN"
fi

APPIMAGE="$DIST/$APP_NAME-$VERSION-linux-$ARCH.AppImage"
rm -f "$APPIMAGE" "$DIST/$APP_NAME-$VERSION-$ARCH.AppImage"
pushd "$DIST" >/dev/null
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$ARCH" VERSION="$VERSION" \
    "$LINUXDEPLOY" --appdir "$APPDIR" \
    -e "$APPDIR/usr/bin/$APP_NAME" \
    -e "$APPDIR/usr/bin/crossh-git" \
    -e "$APPDIR/usr/bin/crossh-note" \
    -e "$APPDIR/usr/bin/crossh-updater" \
    -d "$APPDIR/usr/share/applications/$APP_ID.desktop" \
    -i "$APPDIR/usr/share/icons/hicolor/512x512/apps/$APP_ID.png" \
    --exclude-library='libwayland*.so*' \
    --exclude-library='libxkbcommon*.so*' \
    --exclude-library='libxcb-*.so*' \
    --exclude-library='libXau.so*' \
    --exclude-library='libXdmcp.so*' \
    --output appimage
popd >/dev/null
mv -f "$DIST/$APP_NAME-$VERSION-$ARCH.AppImage" "$APPIMAGE"
if [ ! -f "$APPIMAGE" ]; then
    echo "Error: AppImage 未生成: $APPIMAGE" >&2
    exit 1
fi

# --- 配套安装器 -------------------------------------------------------------
# 生成同目录一键安装脚本 install.sh 与便携安装包（AppImage + install.sh）
INSTALLER="$DIST/install.sh"
cp scripts/install-linux.sh "$INSTALLER"
chmod +x "$INSTALLER"
INSTALL_BUNDLE="$DIST/$APP_NAME-$VERSION-linux-$ARCH-install.tar.gz"
rm -f "$INSTALL_BUNDLE"
tar -C "$DIST" -czf "$INSTALL_BUNDLE" "$(basename "$APPIMAGE")" "$(basename "$INSTALLER")"

echo "==> done:"
echo "    $TARBALL"
echo "    $APPIMAGE"
echo "    $INSTALLER"
echo "    $INSTALL_BUNDLE (AppImage + install.sh)"
