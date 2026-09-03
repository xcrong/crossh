# AppImage 打包显示栈导致异机 GPU 初始化全灭

## 症状

- CI 构建的 AppImage 在构建机之外的发行版上冷启动崩溃：
  `Failed to create surface for any enabled backend: {}`（注意花括号内是空表）。
- 同一源码本地打包的 AppImage 在本机运行正常；`--version` 在坏包上也能正常输出。

## 根因

- wgpu 的 `FailedToCreateSurfaceForAnyBackend` 携带各后端错误表，空表意味着 Vulkan 和 GL
  的 hal 实例在初始化阶段就全部失败（`instance_per_backend` 为空），窗口的 Wayland/X11
  连接本身是好的，死的是 GPU surface。
- `scripts/package-linux.sh` 曾把显示栈（wayland/xkbcommon/xcb/X）连带传递依赖拖入的
  `libffi/glib/z/xml2` 等一起打包；`AppRun` 把 `usr/lib` 置于 `LD_LIBRARY_PATH` 首位，
  旧构建机（如 Ubuntu 22.04）的库盖住本机 Mesa 依赖的系统库，EGL/Vulkan 初始化全灭。
  本机 Mesa 驱动还直连 `expat/lzma/z`，这些同样不能打包。
- “本地打包能跑”只是 bundled 与本机同版本造成的假象，不能作为打包正确的证据。

## 稳定规则

1. AppImage 只携带字体栈（`fontconfig/freetype/harfbuzz`，闭包含 `png/bz2/brotli/graphite`）；
   显示栈与 Mesa 直连库一律排除，交由本机提供。 exclusions 见 `scripts/package-linux.sh`。
2. 验证打包必须用“构建机之外的系统 + 真机 GPU 启动”，`--version` 只能证明包完整，
   不能证明 GPU 路径。

## 验证方法

- 解包坏 AppImage，删显示栈与底层运行时库后直接跑 `squashfs-root/AppRun`，不断言：
  应看到 `Selected GPU adapter` 且无 panic。
- 用缓存的 `appimagetool` 重打包后跑新 AppImage，同样不断言。
- 本次实例：Fedora 44 + AMD 680M，坏包 28 个 bundled 库 → 修后 7 个，点亮。

## 当前实现

- 打包库集合：`scripts/package-linux.sh`（`collect_libs` 种子 + 传递依赖排除表）。

## 搜索关键词

`AppImage`、`bundled libs`、`LD_LIBRARY_PATH`、`FailedToCreateSurfaceForAnyBackend`、
`empty backend`、`libwayland`、`Mesa`、`wgpu`、`surface`
