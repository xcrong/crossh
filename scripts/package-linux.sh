#!/usr/bin/env bash
# 打包 Linux 产物：tar.gz 二进制包 + AppImage（AppDir 组装 + linuxdeploy）
#   + AppImage 便捷安装包（<AppImage 文件名>.tar.gz：AppImage + install.sh）
#   + 发行版原生包（.deb / .rpm，dpkg-deb / rpmbuild 组装）。
#
# 用法:  scripts/package-linux.sh [target] [version]
#         target: x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu（默认按宿主 uname -m 推导）
# 依赖:  rustup、curl、tar、rsvg-convert（Ubuntu 包: librsvg2-bin）、dpkg-deb、rpmbuild（Ubuntu 包: rpm）
# 输出:  dist/crossh-<version>-linux-<arch>.tar.gz、.AppImage、.AppImage.tar.gz、.deb、.rpm 与 install.sh
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
# AppImage 按约定与安装脚本一起再包一层：<AppImage 文件名>.tar.gz。
# 解开即得 AppImage + install.sh，./install.sh 一键安装到桌面。
INSTALLER="$DIST/install.sh"
cp scripts/install-linux.sh "$INSTALLER"
chmod +x "$INSTALLER"
INSTALL_BUNDLE="$APPIMAGE.tar.gz"
rm -f "$INSTALL_BUNDLE"
tar -C "$DIST" -czf "$INSTALL_BUNDLE" "$(basename "$APPIMAGE")" "$(basename "$INSTALLER")"

# --- .deb -------------------------------------------------------------------
# 发行版原生包：dpkg-deb 直接组装，不引入 debhelper。
# FHS 布局：二进制进 /usr/bin，共享资源进 /usr/share/crossh
# （AssetStore::discover 识别 /usr[/local]/share 系统路径）。
# 文件名沿用 tarball 的内核架构（x86_64/aarch64），包内 Architecture 写 Debian 架构。
case "$ARCH" in
    x86_64) DEB_ARCH="amd64" ;;
    aarch64) DEB_ARCH="arm64" ;;
esac
DEB_STAGE="$DIST/deb-$ARCH"
DEB="$DIST/$APP_NAME-$VERSION-linux-$ARCH.deb"
echo "==> deb ($DEB_ARCH)"
rm -rf "$DEB_STAGE"
mkdir -p "$DEB_STAGE/DEBIAN" \
    "$DEB_STAGE/usr/bin" \
    "$DEB_STAGE/usr/share/applications" \
    "$DEB_STAGE/usr/share/icons/hicolor/512x512/apps" \
    "$DEB_STAGE/usr/share/doc/$APP_NAME"
install -m755 "$BIN" "$DEB_STAGE/usr/bin/$APP_NAME"
install -m755 "$GIT_BIN" "$DEB_STAGE/usr/bin/crossh-git"
install -m755 "$NOTE_BIN" "$DEB_STAGE/usr/bin/crossh-note"
install -m755 "$UPDATER_BIN" "$DEB_STAGE/usr/bin/crossh-updater"
bash scripts/copy-shared-assets.sh "$DEB_STAGE/usr/share/$APP_NAME/crossh-assets"
install -m644 README.md "$DEB_STAGE/usr/share/doc/$APP_NAME/README.md"
install -m644 LICENSE "$DEB_STAGE/usr/share/doc/$APP_NAME/LICENSE"
cat > "$DEB_STAGE/usr/share/applications/$APP_ID.desktop" <<EOF
[Desktop Entry]
Name=crossh
GenericName=Terminal Workspace
Comment=Local-first terminal workspace (GPUI)
Exec=crossh
Icon=$APP_ID
Terminal=false
Type=Application
Categories=Development;
StartupWMClass=$APP_ID
Keywords=terminal;workspace;project;git;notes;
EOF
ln -s "$APP_ID.desktop" "$DEB_STAGE/usr/share/applications/crossh.desktop"
ln -s "$APP_ID.desktop" "$DEB_STAGE/usr/share/applications/io.crossh.app.desktop"
rsvg-convert -w 512 -h 512 assets/appicon/icon-master.svg \
    -o "$DEB_STAGE/usr/share/icons/hicolor/512x512/apps/$APP_ID.png"
ln -s "$APP_ID.png" "$DEB_STAGE/usr/share/icons/hicolor/512x512/apps/crossh.png"
ln -s "$APP_ID.png" "$DEB_STAGE/usr/share/icons/hicolor/512x512/apps/io.crossh.app.png"
cat > "$DEB_STAGE/DEBIAN/control" <<EOF
Package: $APP_NAME
Version: $VERSION
Architecture: $DEB_ARCH
Maintainer: xcrong <hi@xcrong.me>
Installed-Size: $(du -sk "$DEB_STAGE/usr" | cut -f1)
Depends: hicolor-icon-theme, libfontconfig1, libfreetype6, libexpat1, libharfbuzz0b, libxcb1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0
Section: utils
Priority: optional
Homepage: https://github.com/xcrong/crossh
Description: Local-first terminal workspace (GPUI)
 Project-oriented local sessions with Git and Notes as pluggable panes.
EOF
cat > "$DEB_STAGE/DEBIAN/postinst" <<'EOF'
#!/bin/sh
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database /usr/share/applications >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi
EOF
cp "$DEB_STAGE/DEBIAN/postinst" "$DEB_STAGE/DEBIAN/postrm"
chmod 755 "$DEB_STAGE/DEBIAN/postinst" "$DEB_STAGE/DEBIAN/postrm"
rm -f "$DEB"
dpkg-deb --build "$DEB_STAGE" "$DEB"

# --- .rpm -------------------------------------------------------------------
# rpmbuild 组装，_topdir 指到 dist 内，不污染 $HOME/rpmbuild。
# 与 .deb 同一套 FHS 文件树（直接复用 $DEB_STAGE/usr），Requires 写
# Fedora/openSUSE 侧包名。CI 需安装 rpm（提供 rpmbuild），见 release.yml。
case "$ARCH" in
    x86_64) RPM_ARCH="x86_64" ;;
    aarch64) RPM_ARCH="aarch64" ;;
esac
RPM_TOPDIR="$DIST/rpm-$ARCH"
RPM="$DIST/$APP_NAME-$VERSION-linux-$ARCH.rpm"
echo "==> rpm ($RPM_ARCH)"
rm -rf "$RPM_TOPDIR"
mkdir -p "$RPM_TOPDIR/SPECS" "$RPM_TOPDIR/root"
cp -a "$DEB_STAGE/usr" "$RPM_TOPDIR/root/"
cat > "$RPM_TOPDIR/SPECS/$APP_NAME.spec" <<EOF
Name:           $APP_NAME
Version:        $VERSION
Release:        1%{?dist}
Summary:        Local-first terminal workspace (GPUI)
License:        GPL-3.0-or-later
URL:            https://github.com/xcrong/crossh
BuildArch:      $RPM_ARCH
Requires:       hicolor-icon-theme
Requires:       fontconfig
Requires:       freetype
Requires:       expat
Requires:       harfbuzz
Requires:       libxcb
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       libwayland-client

%description
Local-first terminal workspace (GPUI): project-oriented local sessions
with Git and Notes as pluggable panes.

%install
mkdir -p %{buildroot}
cp -a $RPM_TOPDIR/root/usr %{buildroot}/

%files
/usr/bin/$APP_NAME
/usr/bin/crossh-git
/usr/bin/crossh-note
/usr/bin/crossh-updater
/usr/share/$APP_NAME/
/usr/share/doc/$APP_NAME/
/usr/share/applications/$APP_ID.desktop
/usr/share/applications/crossh.desktop
/usr/share/applications/io.crossh.app.desktop
/usr/share/icons/hicolor/512x512/apps/$APP_ID.png
/usr/share/icons/hicolor/512x512/apps/crossh.png
/usr/share/icons/hicolor/512x512/apps/io.crossh.app.png

%post
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database /usr/share/applications >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi

%postun
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database /usr/share/applications >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi
EOF
rpmbuild -bb --define "_topdir $RPM_TOPDIR" "$RPM_TOPDIR/SPECS/$APP_NAME.spec"
rm -f "$RPM"
mv -f "$RPM_TOPDIR"/RPMS/"$RPM_ARCH"/"$APP_NAME"-*.rpm "$RPM"

echo "==> done:"
echo "    $TARBALL"
echo "    $APPIMAGE"
echo "    $INSTALLER"
echo "    $INSTALL_BUNDLE (AppImage + install.sh)"
echo "    $DEB"
echo "    $RPM"
