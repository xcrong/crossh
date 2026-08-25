#!/usr/bin/env bash
# 一键打包并安装到本机 /Applications（无需 sudo）
# 用法: scripts/mac_local_install.sh [target] [version]
#       target/version 透传给 scripts/package.sh
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> packaging..."
bash scripts/package.sh "$@"

APP_DIR="dist/crossh.app"
BUNDLE_ID="io.crossh.app"
DEST="/Applications/crossh.app"

if [ ! -d "$APP_DIR" ]; then
    echo "error: $APP_DIR 不存在，打包失败" >&2
    exit 1
fi

echo "==> installing to $DEST (no sudo)"
rm -rf "$DEST"
cp -R "$APP_DIR" /Applications/

# 清 quarantine/provenance 并重签以 bust Gatekeeper 缓存
xattr -cr "$DEST" 2>/dev/null || true
codesign --force --deep --sign - --identifier "$BUNDLE_ID" "$DEST"

echo "==> verifying"
codesign --verify --deep --strict --verbose=2 "$DEST"
echo ""
echo "==> done: $DEST"
echo "    open $DEST"
