#!/usr/bin/env bash
# crossh Linux 安装器：与同目录的 crossh-*.AppImage 配套使用。
# 用法： ./install.sh              # 安装/更新到 ~/.local
#        ./install.sh --system     # 安装到 /usr/local（需 sudo）
APP_ID="me.xcrong.crossh"
APP_NAME="crossh"
DESKTOP_SRC="usr/share/applications/${APP_ID}.desktop"
ICON_SRC="usr/share/icons/hicolor/512x512/apps/${APP_ID}.png"
set -euo pipefail


usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTION]

Options:
  --user        安装到 \$HOME/.local（默认）
  --system      安装到 /usr/local（需 root）
  --uninstall   卸载已安装的 crossh
  --help        显示帮助

说明：
  将与本脚本同目录的 crossh-*.AppImage 安装为桌面应用。
  安装后可在启动器搜索 "crossh" 启动，Dock 将正确显示图标。
EOF
}

MODE="user"
UNINSTALL=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --user) MODE="user"; shift ;;
        --system) MODE="system"; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "未知选项: $1" >&2; usage >&2; exit 1 ;;
    esac
done

if [[ "$MODE" == "system" ]]; then
    PREFIX="/usr/local"
    BIN_DIR="$PREFIX/bin"
    DESKTOP_DIR="$PREFIX/share/applications"
    ICON_DIR="$PREFIX/share/icons/hicolor/512x512/apps"
else
    PREFIX="$HOME/.local"
    BIN_DIR="$PREFIX/bin"
    DESKTOP_DIR="$PREFIX/share/applications"
    ICON_DIR="$PREFIX/share/icons/hicolor/512x512/apps"
fi

TARGET_BIN="$BIN_DIR/${APP_NAME}.AppImage"
TARGET_DESKTOP="$DESKTOP_DIR/${APP_ID}.desktop"
TARGET_ICON="$ICON_DIR/${APP_ID}.png"

do_uninstall() {
    echo "==> 卸载 $APP_ID ($MODE)"
    rm -f "$TARGET_BIN" "$TARGET_DESKTOP" "$TARGET_ICON"
    rm -f "$DESKTOP_DIR/io.crossh.app.desktop" "$DESKTOP_DIR/crossh.desktop"
    rm -f "$ICON_DIR/io.crossh.app.png" "$ICON_DIR/crossh.png"
    rm -f "$ICON_DIR/../scalable/apps/${APP_ID}.svg" 2>/dev/null || true
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        # hicolor 位于 PREFIX/share/icons/hicolor
        gtk-update-icon-cache -f "$(dirname "$ICON_DIR")/.." 2>/dev/null || true
        gtk-update-icon-cache -f "$(dirname "$(dirname "$ICON_DIR")")" 2>/dev/null || true
    fi
    echo "==> 已卸载。如曾手动创建 ~/Applications 副本，请自行删除。"
    exit 0
}

if [[ "$UNINSTALL" -eq 1 ]]; then
    do_uninstall
fi

# 定位脚本所在目录与 AppImage
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 优先匹配 me.xcrong 产物的 AppImage，回退到任意 crossh*.AppImage
APPIMAGE="$(ls -1 "$SCRIPT_DIR"/crossh-*.AppImage 2>/dev/null | head -n 1 || true)"
if [[ -z "$APPIMAGE" ]]; then
    APPIMAGE="$(ls -1 "$SCRIPT_DIR"/*.AppImage 2>/dev/null | head -n 1 || true)"
fi
if [[ -z "$APPIMAGE" || ! -f "$APPIMAGE" ]]; then
    echo "错误：未在 $SCRIPT_DIR 找到 crossh-*.AppImage" >&2
    echo "请将 install.sh 与 AppImage 放在同一目录后重试。" >&2
    exit 1
fi

echo "==> 安装 $APP_NAME ($MODE)"
echo "    AppImage: $APPIMAGE"
echo "    目标: $TARGET_BIN"

mkdir -p "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR"
# 拷贝 AppImage
cp -f "$APPIMAGE" "$TARGET_BIN"
chmod +x "$TARGET_BIN"

# 从 AppImage 解出 desktop 与 icon（优先直接解压，其次回退到 --appimage-extract）
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

EXTRACTED=0
if command -v bsdtar >/dev/null 2>&1; then
    # AppImage 为 squashfs，bsdtar 可直接提取（无需执行）
    if bsdtar -xf "$TARGET_BIN" -C "$TMPDIR" "$DESKTOP_SRC" "$ICON_SRC" 2>/dev/null; then
        EXTRACTED=1
    fi
fi
if [[ "$EXTRACTED" -eq 0 ]]; then
    # 回退：执行 AppImage 的解压（需可执行，已 chmod +x）
    # 在 TMPDIR 中执行，避免污染当前目录的 squashfs-root
    (
        cd "$TMPDIR"
        # 部分环境需 APPIMAGE_EXTRACT_AND_RUN
        APPIMAGE_EXTRACT_AND_RUN=1 "$TARGET_BIN" --appimage-extract "$DESKTOP_SRC" "$ICON_SRC" >/dev/null 2>&1 || \
        APPIMAGE_EXTRACT_AND_RUN=1 "$TARGET_BIN" --appimage-extract >/dev/null 2>&1 || true
    )
    if [[ -f "$TMPDIR/squashfs-root/$DESKTOP_SRC" ]]; then
        EXTRACTED=1
    elif [[ -f "$TMPDIR/squashfs-root/usr/share/applications/${APP_ID}.desktop" ]]; then
        EXTRACTED=1
    fi
fi

if [[ -f "$TMPDIR/squashfs-root/$DESKTOP_SRC" ]]; then
    cp -f "$TMPDIR/squashfs-root/$DESKTOP_SRC" "$TARGET_DESKTOP"
elif [[ -f "$TMPDIR/$DESKTOP_SRC" ]]; then
    cp -f "$TMPDIR/$DESKTOP_SRC" "$TARGET_DESKTOP"
else
    echo "警告：未能从 AppImage 解出 $DESKTOP_SRC，尝试使用 AppDir 回退" >&2
    # 回退：若用户是从源码 dist 直接运行，尝试 dist/*AppDir
    if [[ -f "$SCRIPT_DIR/me.xcrong.crossh.AppDir/$DESKTOP_SRC" ]]; then
        cp -f "$SCRIPT_DIR/me.xcrong.crossh.AppDir/$DESKTOP_SRC" "$TARGET_DESKTOP"
    elif [[ -f "$SCRIPT_DIR/crossh.AppDir/$DESKTOP_SRC" ]]; then
        cp -f "$SCRIPT_DIR/crossh.AppDir/$DESKTOP_SRC" "$TARGET_DESKTOP"
    else
        echo "错误：无法获取 desktop 文件" >&2
        exit 1
    fi
fi
if [[ -f "$TMPDIR/squashfs-root/$ICON_SRC" ]]; then
    cp -f "$TMPDIR/squashfs-root/$ICON_SRC" "$TARGET_ICON"
elif [[ -f "$TMPDIR/$ICON_SRC" ]]; then
    cp -f "$TMPDIR/$ICON_SRC" "$TARGET_ICON"
else
    if [[ -f "$SCRIPT_DIR/me.xcrong.crossh.AppDir/$ICON_SRC" ]]; then
        cp -f "$SCRIPT_DIR/me.xcrong.crossh.AppDir/$ICON_SRC" "$TARGET_ICON"
    elif [[ -f "$SCRIPT_DIR/crossh.AppDir/$ICON_SRC" ]]; then
        cp -f "$SCRIPT_DIR/crossh.AppDir/$ICON_SRC" "$TARGET_ICON"
    else
        # 最后回退：从 AppImage 解出的任意 icon
        FOUND_ICON="$(find "$TMPDIR" -name "${APP_ID}.png" 2>/dev/null | head -n 1 || true)"
        if [[ -n "$FOUND_ICON" ]]; then
            cp -f "$FOUND_ICON" "$TARGET_ICON"
        else
            echo "警告：未找到图标，Dock 可能显示为通用图标" >&2
        fi
    fi
fi


# 修正 desktop 的 Exec 为绝对路径
if [[ -f "$TARGET_DESKTOP" ]]; then
    # 替换 Exec 行，保留 %u/%U 参数
    if grep -q "^Exec=" "$TARGET_DESKTOP"; then
        sed -i "s|^Exec=.*|Exec=$TARGET_BIN %u|" "$TARGET_DESKTOP"
    else
        echo "Exec=$TARGET_BIN %u" >> "$TARGET_DESKTOP"
    fi
    chmod 644 "$TARGET_DESKTOP"
fi

# 兼容旧 ID 的软链（已安装新版后，旧桌面文件仍能启动）
ln -sf "${APP_ID}.desktop" "$DESKTOP_DIR/crossh.desktop" 2>/dev/null || true
ln -sf "${APP_ID}.desktop" "$DESKTOP_DIR/io.crossh.app.desktop" 2>/dev/null || true
if [[ -f "$TARGET_ICON" ]]; then
    ln -sf "${APP_ID}.png" "$ICON_DIR/crossh.png" 2>/dev/null || true
    ln -sf "${APP_ID}.png" "$ICON_DIR/io.crossh.app.png" 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi

echo "==> 完成"
echo "    二进制: $TARGET_BIN"
echo "    Desktop: $TARGET_DESKTOP"
echo "    Icon: $TARGET_ICON"
echo ""
echo "    启动: 在应用列表搜索 'crossh'，或运行: $TARGET_BIN"
echo "    卸载: $(basename "$0") --uninstall"
if [[ "$MODE" == "user" ]] && [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo ""
    echo "提示: $BIN_DIR 不在 PATH，可将以下加入 ~/.bashrc / ~/.zshrc："
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
fi
