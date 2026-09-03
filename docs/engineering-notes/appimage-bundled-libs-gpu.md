# AppImage 打包：显示栈与运行时库走本机，规则交由上游工具

## 症状

- v0.31.0（CI Ubuntu 22.04 构建）在 Fedora 44 上冷启动崩溃：
  `Failed to create surface for any enabled backend: {}`（空表）；同一源码本地打包正常。
- v0.31.1 修显示栈时丢了排除分支的 `continue`，把构建机的 `libc.so.6` 打进包，
  本机新库报 `GLIBC_2.38 not found`，同样起不来。

## 根因

- wgpu 空错误表 = Vulkan 和 GL 的 hal 实例在初始化阶段全灭；窗口的 Wayland 连接本身是好的。
- 旧构建机的 `libwayland/libxcb` 经 `LD_LIBRARY_PATH` 盖住本机 Mesa 依赖的系统库；
  本机 Mesa 驱动还直连 `expat/lzma/z`。本地打包“能跑”只是 bundled 与本机同版本的假象。
- 手写排除名单两次翻车（ Wayland 冲突一次、丢 `continue` 一次）：打包规则不再手写。

## 前人（我们不是第一个）

- `appimage-builder#255`：同一崩溃、同一结论——老系统构建 + 不打包 `libwayland`。
- `pkg2appimage#559` + `mesa#11316` 之后，`libwayland-client.so.0` 进入社区官方 excludelist；
  该表还覆盖 `libc/GL/EGL/drm/xcb/X11/z/expat/uuid`，连 `fontconfig/freetype/harfbuzz` 都不打包。
- pkgforge/sharun 学派（全量打包 + 自带 `ld-linux` + `--library-path`，禁用 `LD_LIBRARY_PATH`）
  是另一条路；我们是终端应用、常拉起子进程，旧 AppRun 的 `LD_LIBRARY_PATH` 会泄漏进用户
  shell，已随本次迁移消除（linuxdeploy 生成的 AppRun 走 rpath）。

## 稳定规则

1. `scripts/package-linux.sh` 只调用 linuxdeploy（+ plugin-appimage）：`-e` 四个二进制、
   `--exclude-library` 排除 `wayland/xkbcommon/xcb/Xau/Xdmcp`；不再手写 collect/排除表。
2. 构建机保持最老被支持版（Ubuntu 22.04，glibc 只向前兼容）；换工具不改变这条铁律。
3. 验证必须用构建机之外的系统 + 真机 GPU 启动；`--version` 只能证明包完整。

## 验证方法

- CI 产物（Ubuntu 22.04 二进制）解包 → linuxdeploy 重组 → `usr/lib` 为空 →
  Fedora 44 + AMD 680M 点亮（`Selected GPU`，无 panic），重打的 AppImage 端到端同样点亮。
- `install-linux.sh` 的解包路径（`usr/share/…desktop/icon`）不受影响，linuxdeploy 原样放置。

## 当前实现

- `scripts/package-linux.sh`（linuxdeploy 本体与插件按 `ARCH` 缓存于 `dist/`，CI 自动下载）。

## 搜索关键词

`AppImage`、`linuxdeploy`、`excludelist`、`libwayland`、`LD_LIBRARY_PATH`、
`FailedToCreateSurfaceForAnyBackend`、`Mesa`、`GLIBC_2.38`、`continue`
