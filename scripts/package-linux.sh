#!/usr/bin/env bash
# 打包 Linux 产物：tar.gz 二进制包 + AppImage（AppDir 组装 + appimagetool）。
#
# 用法:  scripts/package-linux.sh <target> [version]
#         target: x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu
# 依赖:  rustup、curl、tar、rsvg-convert（Ubuntu 包: librsvg2-bin）
# 输出:  dist/crossh-<version>-linux-<arch>.tar.gz 与同前缀 .AppImage
set -euo pipefail

cd "$(dirname "$0")/.."

TARGET="${1:?usage: scripts/package-linux.sh <target> [version]}"
VERSION="${2:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)".*/\1/')}"
APP_NAME="crossh"
DIST="dist"
case "$TARGET" in
    x86_64-*) ARCH="x86_64" ;;
    aarch64-*) ARCH="aarch64" ;;
    *) echo "unsupported target: $TARGET" >&2; exit 1 ;;
esac
STAGE="$DIST/package-linux-$ARCH"
APPDIR="$DIST/$APP_NAME.AppDir"
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

cat > "$APPDIR/usr/share/applications/io.crossh.app.desktop" <<EOF
[Desktop Entry]
Name=crossh
GenericName=Terminal Workspace
Comment=Local-first terminal workspace (GPUI)
Exec=crossh
Icon=io.crossh.app
Terminal=false
Type=Application
Categories=Development;
StartupWMClass=io.crossh.app
Keywords=terminal;workspace;project;git;notes;
EOF
ln -s "usr/share/applications/io.crossh.app.desktop" "$APPDIR/crossh.desktop"
# 兼容旧路径：部分桌面环境仍通过 crossh.desktop 查找
ln -s "io.crossh.app.desktop" "$APPDIR/usr/share/applications/crossh.desktop"

rsvg-convert -w 512 -h 512 assets/appicon/icon-master.svg \
    -o "$APPDIR/usr/share/icons/hicolor/512x512/apps/io.crossh.app.png"
ln -s "usr/share/icons/hicolor/512x512/apps/io.crossh.app.png" "$APPDIR/crossh.png"
# 兼容旧 Icon=crossh
ln -s "io.crossh.app.png" "$APPDIR/usr/share/icons/hicolor/512x512/apps/crossh.png"
ln -s "io.crossh.app.png" "$APPDIR/.DirIcon"

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
SELF="$(readlink -f "$0")"
HERE="${SELF%/*}"
export LD_LIBRARY_PATH="${HERE}/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "${HERE}/usr/bin/crossh" "$@"
EOF
chmod +x "$APPDIR/AppRun"

# 携带运行时库（wayland/xkbcommon/fontconfig/xcb 家族），依赖深度递归收集
collect_libs() {
    local lib path
    for lib in "$@"; do
        path="$(ldconfig -p | awk -v l="$lib" '$1 == l { print $NF; exit }')"
        if [ -n "$path" ] && [ ! -e "$APP_LIB/${path##*/}" ]; then
            cp -L "$path" "$APP_LIB/"
        fi
    done
}

collect_libs \
    libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1 \
    libxkbcommon.so.0 libxkbcommon-x11.so.0 \
    libxcb.so.1 libxcb-render.so.0 libxcb-shape.so.0 libxcb-xfixes.so.0 \
    libxcb-shm.so.0 libxau.so.6 libxdmcp.so.6 \
    libfontconfig.so.1 libfreetype.so.6 libexpat.so.1 libharfbuzz.so.0

for f in "$APP_LIB"/*.so*; do
    [ -e "$f" ] || continue
    while IFS= read -r dep; do
        case "$dep" in
            *ld-linux*|*libc.so.6|*/libm.so*|*/libdl.so*|*/librt.so*|*/libpthread.so* \
            |*libgcc_s.so*|*libstdc++.so*|*libGL*|*libEGL*|*libvulkan*|*libdrm*|*libgbm*)
                continue ;;
        esac
        if [ ! -e "$APP_LIB/${dep##*/}" ]; then
            cp -L "$dep" "$APP_LIB/"
        fi
    done < <(
        LD_LIBRARY_PATH="$APP_LIB" ldd "$f" 2>/dev/null |
            awk '/=> \// { for (i = 1; i <= NF; i++) if ($i == "=>") { print $(i + 1); break } }' |
            sort -u
    )
done

echo "==> appimagetool"
APPIMAGETOOL="$DIST/appimagetool-$ARCH.AppImage"
if [ ! -x "$APPIMAGETOOL" ]; then
    curl -fL -o "$APPIMAGETOOL" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
    chmod +x "$APPIMAGETOOL"
fi
APPIMAGE="$DIST/$APP_NAME-$VERSION-linux-$ARCH.AppImage"
rm -f "$APPIMAGE"
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$ARCH" VERSION="$VERSION" \
    "$APPIMAGETOOL" "$APPDIR" "$APPIMAGE"

echo "==> done:"
echo "    $TARBALL"
echo "    $APPIMAGE"
