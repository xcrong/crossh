# Crossh Roadmap

Crossh 的终端目标是：在保持安全、可预测和跨平台一致性的前提下，支持现代 TUI 应用常用的终端协议。

这里的“完整”指现代 TUI 的通用协议面，不意味着实现每一个终端厂商的全部私有扩展。基础 VT/xterm 能力优先保证广泛兼容，Kitty、iTerm2、Sixel 等协议作为可选增强层。

## 当前状态

已完成的终端协议能力详见 [终端兼容性说明](docs/terminal-compatibility.md)。当前实现已经覆盖：

- 基于 `alacritty_terminal` 的 ANSI/VT/xterm 屏幕、光标、颜色、备用屏幕、重排、鼠标、焦点、粘贴和键盘协议。
- OSC 7 工作目录、OSC 133 Shell 集成、OSC 52 剪贴板策略和 tmux passthrough。
- BEL、OSC 9、OSC 777、Kitty OSC 99 通知，包括分块、替换、关闭、按钮、激活、过期和查询。
- Kitty graphics、iTerm inline image、Sixel，包括分块、图片/布局 ID、Unicode placeholder、相对布局、裁剪、偏移、z-index、删除和 ACK。
- Windows Terminal OSC 9;4 进度状态，以及 CSI 14/16/18/19 尺寸查询。
- 本地与 SSH 会话共用增量协议解析器，并覆盖 PTY 分块、UTF-8 边界和 tmux 嵌套场景。

## P0：下一阶段

这些项目直接补齐当前已支持协议中的高价值缺口。

### Kitty 图形动画

- [ ] 实现动画帧控制：`a=f`、`a=a`、`a=c`。
- [ ] 支持帧持续时间、循环、合成和当前帧切换。
- [ ] 为动画设置总帧数、像素、内存和定时器上限，避免远端输出耗尽资源。
- [ ] 增加分块传输、删除、滚动、备用屏幕和窗口重绘的回放测试。

### 图片传输策略

- [ ] 设计显式的本地文件访问策略，再考虑 Kitty 的文件和临时文件传输（`t=f`、`t=t`）。
- [ ] 评估共享内存传输的跨平台实现；默认继续拒绝不受策略控制的路径和共享内存句柄。
- [ ] 支持 graphics query 返回的有限元数据，并保持响应大小和解析成本有上限。
- [ ] 为自然像素尺寸图片定义准确的光标占位和单元格占用模型。

### 通知平台能力

- [ ] 在 macOS、Linux、Windows 后端统一通知能力抽象。
- [ ] 支持 Kitty 通知图标、声音、urgency 和更精确的 `invisible` 判断。
- [ ] 接入可靠的系统通知关闭回调，并映射到 Kitty 的 close/report 行为。
- [ ] 对不支持某项能力的平台提供稳定降级，不改变应用收到的协议响应格式。

## P1：兼容性与可验证性

### 协议能力协商

- [ ] 建立协议能力矩阵，区分基础 VT/xterm、通用 OSC 扩展和 Kitty/iTerm2/Sixel 私有扩展。
- [ ] 仅在应用或终端请求后发送扩展响应；未知控制序列继续安全忽略。
- [ ] 记录 `TERM`、`COLORTERM` 和可用查询结果，但不把环境变量当作唯一能力证明。
- [ ] 为查询、ACK、错误响应和超时建立统一测试辅助函数。

### 本地、SSH 和复合终端测试

- [ ] 扩充 `tests/fixtures/terminal_compatibility.hex`，覆盖每个协议的跨读取分块场景。
- [ ] 增加 tmux 内嵌、SSH 远端 Shell、备用屏幕和窗口 resize 的组合回放。
- [ ] 继续维护 macOS、Linux、Windows/ConPTY 的 smoke test。
- [ ] 定期执行 `vttest` 和真实 TUI 检查：`tmux`、`nvim`、`fzf`、`btop`、`less`、`lazygit`、`yazi`。
- [ ] 为图形和通知场景补充人工截图检查，避免只验证字节响应而遗漏渲染差异。

### 远程 Shell 集成

- [ ] 设计显式 opt-in 的远程 Shell 集成安装流程，覆盖 Bash、Zsh、Fish 和 PowerShell。
- [ ] 远端安装前显示脚本内容和作用范围，支持卸载和版本升级。
- [ ] 默认不向任意远程 Shell 注入 hook；继续消费远端应用或用户已经启用的 OSC 7/133。
- [ ] 为连接断开、Shell 重启、权限不足和非交互 Shell 提供清晰降级行为。

## P2：长期扩展

- [ ] 补充更多 iTerm2 图片选项和 Sixel 边界行为，并整理与 Kitty graphics 的统一内部模型。
- [ ] 评估 Kitty 的拖放、多光标、鼠标指针和更完整的文本尺寸扩展。
- [ ] 改进高 DPI、字体 fallback、宽字符和组合字符对图片 placeholder 的配合。
- [ ] 增加终端性能指标：解析吞吐、图片解码耗时、帧率、缓存占用和通知延迟。
- [ ] 将协议能力和安全策略暴露为可审计的设置或诊断信息。

## 实现原则

- 基础 VT/xterm 行为优先；厂商扩展不能破坏普通文本终端兼容性。
- 协议解析必须增量、可限流、可取消，并对字符串、图片、通知和嵌套 passthrough 设置明确上限。
- 来自 PTY 的路径、文件名和控制参数一律视为不可信输入。
- 对协议响应使用明确的 ACK、错误码和能力协商，不静默伪造终端能力。
- 每个新增协议能力都要同时提供回放测试、边界测试和兼容性文档。
- 本地与 SSH 路径使用同一协议处理逻辑，差异只保留在 Shell 注入策略和平台通知后端。

## 完成标准

一个路线图项目只有在以下条件都满足后才标记为完成：

1. 有对应的协议解析、状态处理和 UI/relay 行为。
2. 有覆盖分块输入、异常输入和资源上限的自动化测试。
3. 本地和 SSH 路径都经过验证；平台相关行为至少有对应平台的 smoke test。
4. 兼容性说明和安全边界已同步更新。
5. `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 和相关 release 测试通过。

## 参考资料

- [XTerm Control Sequences](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [Kitty protocol extensions](https://sw.kovidgoyal.net/kitty/protocol-extensions/)
- [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- [Kitty desktop notifications](https://sw.kovidgoyal.net/kitty/desktop-notifications/)
