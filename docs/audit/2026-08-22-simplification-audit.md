# Crossh 简化扫描报告（2026-08-22）

触发原因：用户 `@find-simplifications` 手动触发，要求对全仓可简化点进行证据化审计，重点关注死代码 / 重复表示 / 投机泛化 / 手写轮子。

扫描方式：意图基线 `AGENTS.md`、`docs/architecture.md`、全部 ADR（0001-0015）、`docs/engineering-notes/README.md` 与近三轮审计（2026-08-18 架构冗余、2026-08-20 简化、2026-08-21 文档漂移）。分片并行扫描：

- 根 crate feature（workspace/terminal/git/sftp/forwarding/connections）与 `src/shared`、`src/bin`、`src/agent_cli*`
- `crates/crossh-ssh` / `crates/crossh-core` / `crates/crossh-agent` / `crates/crossh-terminal`
- `crates/crossh-ui` / `crates/crossh-ui-component` / `crates/crossh-theme` / `crates/crossh-assets` / `crates/crossh-tui` 与 `src/features/*/view+render`
- settings / updater / infrastructure / scripts / `Cargo.toml` 依赖图

锚点命令：`cargo clippy --workspace --all-targets 2>&1 | rg "dead_code|unused"`（仅 1 处 `unused_variables: cx`）、`rg "allow\(dead_code\)|allow\(unused"` 全仓 16 处（见 S-8）、`rg "shared_text_editing|shlex|glob|split_kv|shell_quote"`、`wc -l` 文件尺寸、`rg "host_entry_matches|available_main_width|clamp_panel_width|terminal_split|format_size|unix_millis|passphrase|SessionMessage|forward::relay"` 逐项核验跨 crate 复用。未改动生产代码，报告本身为文档变更。

## 总体结论

仓库已连续三轮收敛（S-1~S-10、low_latency 与 sdk 镜像均在 2026-08-18/20 闭环），**未发现需要立即删除的整模块或整 crate**。本轮真正的简化空间已从“孤立死代码”转向**三类结构性重复**：

1. **Git 协议辅助层重复**：`run_git`/`git_result`/`git_output` 等命令样板在 `crossh-core` 内 4 个 `git_*` 文件各持一份（S-1，高），`--numstat -z` 的 NUL+Tab 分词与重命名分支在 `git.rs:514` 与 `git_history.rs:272` 双实现（S-2，中），`field()` 的 `trim_matches(['\r','\n'])` 在三文件逐字等价。
2. **跨 crate 的同构算法**：`MAX_OUTPUT_BYTES=24KiB` 的 `append_output` 截断在 `crossh-core/src/commands.rs:706`（`Arc<Mutex<String>>`）与 `crossh-ssh/src/connection.rs:699`（`&mut String`）字节级等价（S-3，中）；`shell_quote` 本地已委托 `shlex::try_quote` 而远程 `shell_quote_remote` 仍手写单引号转义且 NUL 行为不一致（S-4，中，安全边界）。
3. **已抽离组件的残留镜像**：`host_entry_matches` 在 `sidebar.rs:26` 与 `shell.rs:1565` 逐字重复（S-5，中），`available_main_width` 在 `shell.rs:1557` 残留 `#[allow(dead_code)]` 死克隆，与 `crossh-ui-component/src/panel.rs:59` 的权威实现双真相且测试亦双份（S-6，低），`terminal_split_available/left_width` 的 `clamp(min,max)` 几何与 `panel.rs:51 clamp_panel_width` 同义（S-7，低-中）。其余为低危单点：`AuthChoice::Key.passphrase` 死字段（S-8）、过期 `allow(...)` 抑制（S-9）、`SshConfig.hosts` 字段+getter 并存（S-10）、`format_size` 双层 shim（S-11）、`unix_timestamp`/`unix_millis` 双时间 helper（S-12）。

已验证无问题的受保护表面：`crossh-ssh` 的 `run_connection select!` 生命周期与 `WeakEntity` 池、`known_hosts` 决策链、`crossh-update` 的 `UpdateManifest.signature`（ADR 0014）、`crossh-theme` 的零依赖 token 与 `crossh-ui/theme` 的 GPUI 适配、`crossh-assets` 的 `IconName` 嵌入、`check-architecture.sh` 的 `terminal_element.rs:2196` 白名单与零 `gpui` 污染（`logic crates import GPUI` 全绿）。`shared/text_editing` 与 `shared/input_handler` 在 2026-08-20 已完成抽离，本轮未发现新的 IME/按键分发重复。

## 发现

| 编号 | 问题 | 严重度 | 证据与消费者结论 |
| --- | --- | --- | --- |
| S-1 | `crossh-core` 内 Git 命令样板 6 份拷贝 | 中 | `crates/crossh-core/src/git.rs:357 run_git_paths` / `367 run_git` / `413 git_result` / `503 git_output` / `555 git_output_limited` / `588 git_stdout` / `596 command_error` 手写同一套 `Command::new("git").arg("-C").arg(cwd).args(..).output()` + `stderr -> GitError::CommandFailed` 映射；`crates/crossh-core/src/git_branch.rs:59 run_git`、`:138 field`、`crates/crossh-core/src/git_history.rs:130` / `:320 field`、`crates/crossh-core/src/git_stash.rs:113` / `:131 field`、`crates/crossh-core/src/git_status.rs:86 git`、`crates/crossh-core/src/git_conflict.rs:40 run_git_paths` 各自重复。`field()` 三份完全等价 `String::from_utf8_lossy(...).trim_matches(['\r','\n'])`。消费者：生产（Git 视窗/历史/分支/stash/状态栏全量调用，非死代码）；问题是维护成本——改异常文案需改 5 处，`GIT_OPTIONAL_LOCKS=0` 与 `-C` 前缀分散。**保留论点**：`crossh-core` 已贴近 2000 行但 helpers 为纯 `std::process`，可抽 `git/command.rs` 统一 `run_git(cwd,args)->Result<Vec<u8>,GitError>` 仍满足“logic 不依赖 UI”与文件尺寸约束。 |
| S-2 | `--numstat -z` NUL 解析双实现 | 中 | `crates/crossh-core/src/git.rs:514 numstat_map(&[u8])->HashMap` 与 `crates/crossh-core/src/git_history.rs:272 parse_numstat(&[u8])->Vec<CommitFileChange>` 共享 `split(\|b\| *b==0) -> splitn(3,\|b\|*b==b'\t') -> path.is_empty() 则消费后两条为 old/new` 与 `b"-" -> 0` 的 `parse_count`（`git.rs:706` vs `git_history.rs:313`）。差异仅输出容器。消费者：生产（工作区改动计数 vs 单提交文件列表）。**保留论点**：形状不同（聚合 Map vs 列表）保留分离有理由，但 NUL/Tab 分词+重命名分支应为共享迭代器，避免一处修 whitespace/binary 另一处遗漏。 |
| S-3 | 24 KiB 输出截断 `append_output` 双拷贝 | 中 | `crates/crossh-core/src/commands.rs:19 MAX_OUTPUT_BYTES=24*1024` + `706-720 append_output(Arc<Mutex<String>>)` 与 `crates/crossh-ssh/src/connection.rs:621 MAX_REMOTE_COMMAND_OUTPUT=24*1024` + `699-710 append_remote_output(&mut String)` 字节级等价：`push_str(String::from_utf8_lossy)` → `len>limit` → `char_indices().find(\|(i,_)\|*i>=start)` → `drain(..start)`。测试亦重复 UTF-8 边界用例（`commands.rs:649,698` vs `connection.rs:878-887`）。消费者：生产（`BackgroundTaskManager::run_background_process` 与 `run_remote_command` 共用）。已有 `crossh-core::format` 下沉先例，可抽 `fn truncate_to_limit(s:&mut String, limit:usize)` 供两者复用。**保留论点**：并发容器不同（Mutex vs &mut）导致签名分化，但截断算法本身纯字符串，不涉及 tokio/GPUI 隔离。 |
| S-4 | `shell_quote` 本地已轮子化、远程仍手写（安全边界） | 中 | 本地 `crates/crossh-core/src/terminal/shell.rs:426 shell_quote` 已委托 `shlex::try_quote`（`Cargo.toml:10 shlex=1`，覆盖空串/换行/`'`/`\0`，NUL 回退手写），远程 `crates/crossh-ssh/src/connection.rs:712 shell_quote_remote` 仍 `format!("'{}'", v.replace('\'', "'\\''"))`，调用点 `connection.rs:642-644 "cd -- {} && exec sh -lc {}"` 未处理 NUL/空串。消费者：生产（本地 ZDOTDIR/XDG 与远程 `run_remote_command`）。**保留论点**：`crossh-ssh` 未声明 `shlex` 依赖是唯一阻碍，补依赖即消；远程 cwd/command 可含 `'`，手写版对 NUL 的不一致属真实缺陷，非风格问题。 |
| S-5 | `host_entry_matches` 逐字重复 | 中 | `src/features/workspace/sidebar.rs:26-31` 与 `src/features/workspace/shell.rs:1565-1568` 为逐字相同 `entry.alias.to_ascii_lowercase().contains(query) \|\| entry.detail.to_ascii_lowercase().contains(query)`；调用点 `sidebar.rs:80`（过滤渲染）与 `shell.rs:905`（`position(\|entry\| host_entry_matches(entry,&query_lower))`）。消费者：生产（主机搜索过滤）。侧栏本就是主机列表 owner（`sidebar.rs:655 local_dir_matches_query` 已在同文件），应上移收敛为 `connections/host.rs` 或 `sidebar` 的 `pub(crate)`。**保留论点**：两文件属同一 feature，但路径正确性需确认 `HostEntry` 归属（`connections/host.rs` 已有 `HostEntry`）。 |
| S-6 | `available_main_width` 死克隆与双真相测试 | 低 | `src/features/workspace/shell.rs:1555-1563 #[allow(dead_code)] fn available_main_width(viewport_width, sidebar_width, quick_commands_width)->Pixels { px((...).max(0.)) }` 与 `crates/crossh-ui-component/src/panel.rs:59 pub fn available_main_width(...)` 签名与实现完全等价；`shell_render.rs:98` 已改调 `crossh_ui_component::panel_available_main_width`，`shell.rs` 版本仅被自身测试 `shell.rs:1669-1672` 消费。消费者：`panel.rs:59` 为权威生产实现（2 处渲染调用 + 4 项单测 `panel.rs:492-496`），`shell.rs:1557` 为非生产（测试独占，生产零调用），属过期 `allow(dead_code)`。**保留论点**：`shell.rs:1556` 注释"保留独立函数供后续 split 响应式重构复用"已由 `panel.rs` 接管，删除无行为差异，仅减 1 个死符号与 4 行重复测试。 |
| S-7 | `terminal_split_*` 几何与 `panel::clamp_panel_width` 同义 | 低-中 | `src/features/workspace/view.rs:321 terminal_split_available(Pixels)->bool`（`width >= px(328.)`）、`325 terminal_split_left_width(requested,default,min,max)->f32`（`value.clamp(min,max)` + 哨兵 `0.` 回退）、`35-36 TERMINAL_SPLIT_MIN_PANE_WIDTH=160 / HANDLE_WIDTH=8` 与 `crates/crossh-ui-component/src/panel.rs:51 clamp_panel_width` / `59 available_main_width` / `SidePanel::resolved_width` 同为 `clamp(min,max)+max(0)` 几何；`shell_render.rs:81/90/98` 已收敛到 `panel.rs`，`view.rs:182-331` 的 split 仍自持一套。消费者：生产（分栏拖拽与布局）。**保留论点**：`terminal_element.rs` 2186 行白名单不触碰，但分栏几何应收敛到 `panel.rs` 单一实现（建议 `terminal_split_left_width` 改调 `clamp_panel_width`，`terminal_split_available` 改调 `available_main_width` 阈值），消除三处并存。 |
| S-8 | `AuthChoice::Key.passphrase` 死字段 | 低-中 | `crates/crossh-ssh/src/session.rs:15-19 pub enum AuthChoice::Key { user, path, passphrase: Option<String> }`，`session.rs:31-72 default_auth_for` 始终 `passphrase: None`（`:43`），`connection.rs:511 match AuthChoice::Key{user,path,..}` 忽略该字段，`rg passphrase` 全仓仅定义处与 1 条错误文案。真实口令走 `CredentialKind::Passphrase` 的交互式 `request_credential`（`connection.rs:586`）。消费者：类型 `pub` 属模糊语料，但生产零通路。**保留论点**：若保留需打通 `load_secret_key(path, passphrase)` 预填路径，否则应删字段收缩 `AuthChoice` 至真实能力，避免误导调用方以为可预传口令。 |
| S-9 | 过期 `#[allow(dead_code)]` / `#[allow(unused_imports)]` 抑制 | 低 | `src/agent_cli_input.rs:3 #[allow(dead_code)]`（整块 `input` 模块仅被 `#[cfg(test)] use input::{delete_previous_char,...}` 消费，生产零 import）、`crates/crossh-agent/src/session.rs:203 #[allow(dead_code)] struct SessionMessage<'a>`（自 `save_session` 改用 `serde_json::json!` 后零构造，`rg SessionMessage` 仅定义处）、`crates/crossh-agent/src/manager.rs:9 #[allow(unused_imports)] use std::fs`（仅测试 `manager.rs:235 fs::read_dir` 使用，生产构建确未用）、`src/agent_cli_slash.rs:35 allow(dead_code)`、`src/features/workspace/registry.rs:318` 等与 `quick_commands_rail.rs:276` / `shell.rs:1556` 并存。消费者：非生产/历史遗留。**保留论点**：`git/mod.rs:70/82/93/103/113` 的 5 处 `visual-tests` 夹具与 `git_launcher.rs:34/40/52` 的双 binary `#[path]` 挂载为 ADR 0008 保护的必要豁免，应保留并已补充注释；本条仅指上述 3 处过期抑制。 |
| S-10 | `SshConfig.hosts` 字段与 getter 并存 | 低 | `crates/crossh-core/src/config/ssh_config.rs:63 pub hosts: Vec<HostConfig>` 与 `:83-85 pub fn hosts(&self)->&[HostConfig]` 同名并存，调用点多为直接字段 `ssh_config.rs:450-459` 测试 `c.hosts.len()`，外部 `src/features/connections/manager.rs` 等可任选其一。消费者：`pub` 属模糊，但两者均无封装收益。**保留论点**：若 `hosts` 需保持 `pub` 供序列化仅留字段，若需封装应改为私有字段+getter，二选一。 |
| S-11 | `format_size` 双层 shim 残留 | 低 | `crates/crossh-core/src/format.rs:26 pub fn format_size -> format_bytes` 仅被自身测试 `format.rs:52` 调用，生产零调用；`src/features/sftp/logic.rs:155 pub(crate) fn format_size -> format_bytes` 被 `render.rs:98,322` 生产调用。两者均为历史过渡 shim，`crossh-ssh/src/sftp.rs:10` 已直连 `format_bytes`。消费者：`format.rs:26` 非生产，`logic.rs:155` 生产但可直呼 `format_bytes`。**保留论点**：若需对外保持 `format_size` 兼容仅保留 `crossh-core` 一处即可，`logic.rs`/`render.rs` 直呼 `format_bytes` 更清晰，无行为差异。 |
| S-12 | `unix_timestamp` / `unix_millis` 双时间 helper | 低 | `crates/crossh-core/src/commands.rs:377 fn unix_timestamp()->u64 { as_secs() }`（调用点 `:138,220` 命令历史 `last_used` 秒级）与 `crates/crossh-agent/src/session.rs:659 fn unix_millis()->u64 { as_millis() }`（调用点 `:66,81,86,136` 会话 `updated_at` 毫秒级）单位不同但 `SystemTime::now().duration_since(UNIX_EPOCH)` 前缀完全等价。消费者：生产（命令历史 vs 会话树）。**保留论点**：单位差异可保留双函数，但应在同一 crate（`crossh-core::format` 或 `process.rs`）提供 `unix_secs`/`unix_millis` 共享，避免 `agent` 侧私有拷贝与 `commands.rs` 重复 `dirs::home_dir` 语义的离散维护。 |

已验证无问题的受保护表面（避免重复立项）：

- `crossh-ssh` 的 `run_connection select!` + `WeakEntity` 池、`known_hosts` 决策链、`crossh-agent` 的 `SessionEntry{parentId}` 树与 `MessageQueue/EventBus`、`crossh-update` 的 `signature` 二级字段（`model.rs:25,37` 为 ADR 0014 契约）、`crossh-theme`→`crossh-ui/theme` 的透传（22 个 `color()` 包装为 ADR 0003 的有意隔离，禁止合并）、`shared/text_editing.rs` 的 `handle_text_editing_key` 与 `shared/input_handler.rs` 的 `StringField/EditingField`（2026-08-20 已收敛）、`terminal_element.rs:2196` 白名单、Lucide 1.27.0 与 Zed `1b04e4c...` 固定 revision。

## 处置 Backlog

| 优先级 | 编号 | 建议处置 | 说明 |
| --- | --- | --- | --- |
| P1 | S-1 | 直接修（小合并） | 新建 `crates/crossh-core/src/git/command.rs` 统一 `run_git(cwd,args)->Result<Vec<u8>,GitError>`（含 `-C`、`GIT_OPTIONAL_LOCKS=0`、`stderr` 映射），`git.rs`/`git_branch.rs`/`git_history.rs`/`git_stash.rs`/`git_status.rs`/`git_conflict.rs` 各自 `run_git`/`field`/`command_error` 删除，复用单一实现。`git.rs` 已 500+ 行，抽离后 `git_branch/stash/history` 各减 15-20 行，`cargo test -p crossh-core --lib git*` 为门禁。 |
| P1 | S-2 | 直接修（纯函数抽离） | 抽 `crates/crossh-core/src/git/numstat.rs` 的 `fn parse_numstat_nul(output:&[u8], mut on_entry: impl FnMut(..))` 共享 NUL/Tab 分词+重命名分支，`git.rs:514 numstat_map` 与 `git_history.rs:272 parse_numstat` 仅保留容器差异。附带 `parse_count` 统一为 `fn parse_count(b:&[u8])->u64`。 |
| P2 | S-4 | 直接修（补依赖） | `crates/crossh-ssh/Cargo.toml` 增 `shlex = "1"`（与根同版本），`connection.rs:712 shell_quote_remote` 改委托 `shlex::try_quote` 并在 NUL 失败时回退原单引号转义（与 `terminal/shell.rs:426` 已落地形态一致），`connection.rs:643` 的 `cd --` 拼接语义随之统一。`cargo test -p crossh-ssh remote_shell_quote*` 为回滚哨兵。 |
| P2 | S-3 | 直接修（下沉到 `crossh-core`） | 在 `crates/crossh-core/src/format.rs` 或 `process.rs` 新增 `pub fn truncate_to_limit(s:&mut String, limit:usize)`，`commands.rs:706` 与 `connection.rs:699` 均委托，`MAX_*_OUTPUT` 常量保留各自 24 KiB 语义仅算法共享。两处测试的 UTF-8 边界用例可合并为 `format::tests::truncate_preserves_char_boundary`。 |
| P2 | S-5 | 直接修 | 将 `host_entry_matches` 上移至 `crates/crossh-core/src/connection.rs` 或 `src/features/connections/host.rs` 的 `impl HostEntry::matches_query`，`sidebar.rs:26` 与 `shell.rs:1565` 删除私有拷贝，`sidebar.rs:80` 与 `shell.rs:905` 改调 `entry.matches_query`。与 `shell.rs` 的响应式重构同批做。 |
| P2 | S-6 | 直接修（删死代码） | 删除 `src/features/workspace/shell.rs:1555-1572 available_main_width` 及其 4 行测试 `shell.rs:1669-1672`，保留 `crates/crossh-ui-component/src/panel.rs:59` 权威实现；`shell.rs:1624` 的 `QuickCommandsPanelMode, available_main_width` 导入随之精简为仅 `QuickCommandsPanelMode`。验证 `cargo test --workspace` 中 `panel::available_main_width_truncates_at_zero` 仍绿。 |
| P3 | S-8 | spec 或直接修（需裁定） | 若 `passphrase` 预填无产品计划，直接删除 `AuthChoice::Key.passphrase` 字段（`session.rs:18`）并精简 `default_auth_for` 的 `Key { user, path }` 形态；若保留预填需补 `load_secret_key(path, passphrase)` 通路并补 spec。`rg passphrase` 全仓仅 2 命中，删除成本低。 |
| P3 | S-9 | 直接修（清抑制） | 删除 `agent_cli_input.rs:3` 的 `#[allow(dead_code)]`（或改为 `#[cfg(test)]` 模块）、`session.rs:203-210 SessionMessage` 死结构、`manager.rs:9` 的 `#[allow(unused_imports)]`（改为测试模块内 `use std::fs`）、`agent_cli_slash.rs:35` 过期 allow。`git/mod.rs:70-113` 5 处与 `git_launcher.rs:34/40/52` 3 处为必要豁免，保留并已注释。`cargo clippy --workspace --all-targets` 零 `allow(dead_code)` 过期为门禁。 |
| P3 | S-10 | 直接修 | 二选一：`ssh_config.rs:63 pub hosts` 改为私有 `hosts: Vec<HostConfig>` 仅留 `hosts()` getter，或删除 getter 仅留字段；同步更新 `ssh_config.rs:450-459` 测试的直接字段访问。`cargo test -p crossh-core --lib config::ssh_config` 为门禁。 |
| P3 | S-11 | 直接修（删一层 shim） | 删除 `crates/crossh-core/src/format.rs:26 format_size` 别名（保留 `format_bytes` 权威），`src/features/sftp/logic.rs:155` 的 `format_size` 若需兼容仅保留一处或让 `render.rs:98,322` 直呼 `format_bytes`。`cargo test -p crossh-core --lib format` 3 项仍绿。 |
| P3 | S-12 | 直接修（可选） | 在 `crates/crossh-core/src/format.rs` 提供 `pub fn unix_timestamp_secs()` / `pub fn unix_timestamp_millis()`，`commands.rs:377` 与 `session.rs:659` 删除私有拷贝，`crossh-agent` 侧 `use crossh_core::unix_timestamp_millis`。保留双函数但单一定义点。 |
| P3 | S-7 | 随功能变更顺带 | `view.rs:321 terminal_split_available` 改调 `panel::available_main_width` 阈值判断，`325 terminal_split_left_width` 改调 `clamp_panel_width`，`shell.rs:1556` 已删后 `view.rs` 为唯一剩余几何。`workspace/view.rs:1443-1452` 的 split 单测同步更新。不单独立项，随 split 响应式重构顺带。 |

受保护表面中"有意保留"项（本轮不进入 backlog）：

- `crossh-theme`→`crossh-ui/theme` 的 22 个透传、`crossh-assets`→`crossh-ui/icons` 的双层图标、`crossh-tui` 的 `visible_width/wrap_text_with_ansi` 与 `ScreenRenderer` 的 `BEGIN_SYNC/OSC52` 管线、`Button::hover_background` / `BadgeTone` 等多页语义分发、`ToggleSwitch/Stepper/Select` 的单页设置控件（`settings/window.rs` 唯一消费者但为可访问性契约）、`Sftp download/upload` 的进度循环与 `forward::relay_channel_tcp` 的 `-L/-D/-R` 分离（监听+停止可抽 `run_tcp_listener` 但属行为边界，不强制）。

## 与 SDD 工作流的衔接

- **直接修类（豁免清单内）**：S-1~S-6、S-9~S-12 均为“文档漂移、死抑制清理、一行修复、纯函数抽离/下沉、共享常量收敛”，无行为变更（或仅收缩未暴露的 `pub` 字段），可直接修并以 `cargo fmt --check` / `scripts/check-architecture.sh` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace`（含 `env -u CROSSH_UPDATE_SIGNING_KEY` 干净环境）验证。S-1/S-2 涉及 `crossh-core` 内 Git 辅助层，需在 `git/command.rs` 或 `git/numstat.rs` 抽离时保留 `NUL`/`0`/`-` 的边界测试。
- **需 spec 认领类**：S-8 的 `passphrase` 字段删除涉及 `AuthChoice` 公开类型的收缩，若确认无“预填口令”产品计划可直接修；若需保留预填能力则立短 spec（`20260817-remove-auth-choice-password.md` 已为 `in-progress`，可扩展范围至 `Key.passphrase` 的打通或删除裁定）。
- **决策类（ADR）**：无。本轮未发现需新增 ADR 的结构性边界变更；`terminal_split` 几何收敛到 `panel.rs` 属组件内聚，不触碰 ADR 0007/0013 的 workspace 组合契约。

## 本轮未决项

- 未重新执行全量 `cargo test --workspace` 与 `cargo clippy --workspace --all-targets` 的 `-D warnings` 门禁（离线缓存可补跑），死代码结论基于 `rg` 静态引用与分片交叉验证 + `cargo clippy` 的 `dead_code|unused` 锚点（仅 1 处 `unused_variables: cx`）。
- S-1/S-2 的 `git/command.rs` 与 `git/numstat.rs` 抽离需确认 `git.rs:357 run_git_paths` 的 `paths: &[String]` 追加语义与 `git_output_limited` 的 `MAX_DIFF_BYTES` 截断是否纳入统一 helper。
- S-6 已验证 `panel.rs:59 available_main_width` 权威实现与 `shell.rs:1557` 死克隆的测试等价性（`px(700.)/216./240. -> px(244.)` 等 4 组断言完全等价），删除后仅保留 `panel::tests` 单源。
- S-9 的 `agent_cli_input.rs:3` 整模块 `allow(dead_code)` 是否改为 `#[cfg(test)] mod input` 需与 `src/agent_cli.rs:34 #[path]` 的测试导入形态一并裁定。
- 与历史审计衔接：`2026-08-20` 的 S-1~S-10（含 `input_handler`/`text_editing_key`/`shlex`/`glob+shellexpand`/`unicode-width`/`format.rs`/`settings_actions`）已全部完成；`2026-08-18` 的 S-1（sdk 镜像）与 S-2（low_latency 残骸）在 `20260818-sdk-single-source-and-lowlatency-removal.md:done` 已删除，S-4 已由 `crossh-core::format` 下沉完成；本文 S-6 的 `host_entry_matches` 为 `2026-08-18` S-3 的延续，S-11 为 S-4 的 shim 尾巴。

每条候选的最强反方论证（为什么保留）：

- S-1：`crossh-core` 已 5923 行，合并 helpers 会使单文件逼近 2000 行红线？——但 `git/command.rs` 为新文件而非向 `git.rs` 堆叠，且 `git_branch.rs` 等本就独立，抽离反而分散行数压力。
- S-2：`numstat_map` 与 `parse_numstat` 输出容器不同（Map vs Vec）保留分离更直观？——容器差异仅在收集端，分词与重命名分支为纯字节逻辑，共享迭代器不改变调用点可读性。
- S-3：`Arc<Mutex<String>>` 与 `&mut String` 并发容器不同，强行统一会引入泛型噪音？——截断算法可为 `fn truncate_to_limit(s:&mut String, limit:usize)` 纯函数，调用方各自 `lock` 后委托，无泛型。
- S-4：`crossh-ssh` 补 `shlex` 依赖会增加传递足迹？——`shlex` 为零依赖纯字符串 crate，足迹可忽略，且与 `crossh-core` 已依赖版本一致，安全收益大于体积。
- S-5：`sidebar.rs` 与 `shell.rs` 分属 workspace 内不同职责，私有 helper 重复无害？——搜索过滤是主机域语义，应归 `HostEntry` 自身，`pub(crate) fn matches_query` 可消除隐式同步成本。
- S-6：`shell.rs` 的 `available_main_width` 注释"供后续 split 响应式重构复用"属前瞻预留？——`panel.rs:59` 已为响应式提供 `available_main_width` 与 `clamp_panel_width` 且被 `shell_render.rs:98` 实际消费，预留已兑现，死克隆无保留必要。
- S-7：`terminal_split` 的 `160/8` 阈值与 `panel` 的 `216/360` 语义不同，合并会混淆常量？——阈值常量保留在 `view.rs`，仅 `clamp`/`max(0)` 算法委托 `panel.rs`，常量不混。
- S-8：`passphrase` 为未来"预填口令"留的扩展点，删后难回退？——字段 `pub` 且全仓零通路，保留反而误导调用方，未来真需预填应由 spec 重新设计 `AuthChoice` 形态而非静默字段。
- S-9：`allow(dead_code)` 为 `visual-tests` / 双 binary 挂载的必要豁免，误删会引入 `cargo check` 噪音？——本条仅指 `input`/`SessionMessage`/`manager.rs fs` 三处过期抑制，`git/mod.rs` 与 `git_launcher.rs` 的必要豁免已验证保留。
- S-10：`pub hosts` 字段供序列化直接访问，getter 供封装，保留冗余便于迁移？——`SshConfig` 未派生 `Serialize`，字段直接访问与 getter 无迁移差异，二选一即可。
- S-11：`format_size` 别名保持对旧调用点 `render.rs` 的兼容？——`render.rs:98,322` 仅 2 处且为 crate 内调用，直呼 `format_bytes` 无兼容负担。
- S-12：`as_secs` 与 `as_millis` 单位不同，强行统一会引入单位混淆？——保留双函数但单一定义点（`unix_secs`/`unix_millis`）恰为避免单位混淆，调用点语义更清晰。
