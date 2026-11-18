# workspace-state Delta

## Purpose

定义 daemon 持有的持久工作区：项目、tab、pane 与分屏树的稳定身份，跨 GUI 与 daemon 重启的恢复语义，修改回执与并发仲裁，pane 与 session 的生命周期关系，运行状态与错误的可观察字段，旧设置的一次性迁移，以及前端通过工作区快照读写布局的方式。

## ADDED Requirements

### Requirement: 工作区由常驻进程持有并串行仲裁

项目、tab、分屏树、比例、焦点与活跃位置 MUST 由常驻进程持有。它们的存续 MUST 不依赖任何 GUI 进程是否运行；GUI、CLI 等前端 MUST 只通过工作区快照读取状态，并通过工作区操作修改状态。所有修改 MUST 由常驻进程串行应用，应用后 MUST 通知所有已连接客户端。

#### Scenario: GUI 退出后工作区仍在

- **WHEN** 用户打开了多个项目和 tab 后关闭 GUI
- **THEN** 常驻进程仍然报告同一组项目、tab、pane 与活跃位置
- **AND** 之后启动的任意前端看到的是这棵树，而不是空工作区

#### Scenario: 两个客户端看到同一棵树

- **WHEN** 两个客户端同时连接到常驻进程，其中一个新建分屏
- **THEN** 两个客户端最终都收到包含该分屏的同一份工作区快照
- **AND** 两侧的项目、tab、pane 顺序与焦点一致

### Requirement: 项目归属与 shell cwd 解耦

pane 的项目归属 MUST 由工作区树决定。shell 内的工作目录变化 MUST 只更新该 pane 的当前 cwd，MUST NOT 改变它所属的项目、tab 或分屏位置。

#### Scenario: 在项目子目录中工作

- **WHEN** 属于 `/repo` 项目的 pane 在 shell 中切换到 `/repo/crates/foo`
- **THEN** 该 pane 仍然属于 `/repo` 项目
- **AND** GUI 重启后它仍在原来的 tab 和分屏位置，不会出现新的 `/repo/crates/foo` 项目

#### Scenario: 同一项目的不同 cwd

- **WHEN** 同一项目下的两个 pane 分别位于不同 cwd
- **THEN** 重启后它们仍显示在同一个项目、同一个 tab 的各自分屏中
- **AND** 侧栏项目数量不因 cwd 数量增加

### Requirement: 布局身份跨重启稳定

项目、tab、pane 与分屏节点 MUST 拥有由常驻进程生成并持久化的稳定身份。这些身份 MUST 在 GUI 重启和常驻进程重启后保持不变。会话身份 MUST 只在常驻进程生命周期内有效，MUST NOT 被持久化，也 MUST NOT 被用来恢复布局。

#### Scenario: GUI 重启后身份不变

- **WHEN** 常驻进程仍在运行时重启 GUI
- **THEN** 每个 pane 的布局身份不变
- **AND** 仍在运行的会话身份不变，GUI 重新接入这些会话而不是新建 shell

#### Scenario: 常驻进程重启后 pane 身份不变

- **WHEN** 常驻进程重启
- **THEN** 项目、tab、pane 与分屏节点的身份与重启前一致
- **AND** 会话身份全部更换，新会话绑定到原有 pane

### Requirement: 工作区持久化、耐久性与损坏恢复

工作区 MUST 持久化到用户配置目录。写入 MUST 使用原子替换，并 MUST 保留至少一份上一版可用状态。结构变更（项目、tab、pane、分屏、活跃位置、顺序与名称）MUST 在返回修改回执前完成原子写入与 fsync；比例、焦点、cwd 与尺寸 MAY 合并写入，回执在入队后返回。正常停止或显式 flush 之后重启 MUST 完整恢复布局；被强制结束或崩溃时，已回执且未携带存储错误的结构变更 MUST NOT 丢失，未回执的视为未发生，最多丢失最近 500 ms 的布局元数据。

#### Scenario: 结构变更后重启

- **WHEN** 用户打开项目、新建 tab、建立分屏并移动焦点后正常停止并重启常驻进程
- **THEN** 重启后项目、tab 顺序、分屏树、比例、焦点与活跃位置与重启前一致

#### Scenario: 崩溃最多丢失元数据

- **WHEN** 常驻进程在最后一次比例调整后 500 ms 内被强制结束
- **THEN** 重启后比例可能回退到上一次持久化的值
- **AND** 此前的项目、tab、分屏结构与 tab 顺序完整保留

#### Scenario: 写入中断不产生半份文件

- **WHEN** 常驻进程在写入工作区文件的过程中被强制结束
- **THEN** 下次启动读到的是完整的上一版或完整的新版本
- **AND** 不会因为截断的 JSON 而丢失全部项目

#### Scenario: 主文件损坏

- **WHEN** 主工作区文件无法解析而备份可用
- **THEN** 常驻进程从备份恢复工作区
- **AND** 损坏文件被保留以供排查，而不是被静默覆盖
- **AND** 主文件与备份都损坏时，两者被改名隔离后以空工作区启动

### Requirement: 持久化失败与格式过新的可观察性

持久化写失败 MUST NOT 阻止工作区继续在内存中服务；失败状态 MUST 经快照与回执暴露，并在成功后清除。结构变更已应用但写盘失败时，回执 MUST 为 `Applied`、`durable=false` 并携带存储错误；元数据变更的回执 MUST 为 `Applied`、`durable=false` 且不携带存储错误；`Unchanged`、`Conflict` 与 `Rejected` MUST NOT 改变持久性。格式版本高于当前版本时，常驻进程 MUST 拒绝加载、保留文件、不发布端口文件，并以非零状态退出；退出前 MUST 尽力写下 `daemon.error`，成功启动 MUST 删除它，删除失败只记日志。`daemon.error` 的展示前提见「协议版本与能力不匹配时明确拒绝」。

#### Scenario: 持久化写失败仍可服务

- **WHEN** 工作区文件所在目录暂时不可写
- **THEN** 常驻进程继续接受修改并在内存中应用
- **AND** 快照携带存储错误，客户端可以显示该错误
- **AND** 目录恢复可写后，下一次成功写入包含期间的全部修改，错误从快照消失

#### Scenario: 结构变更写盘失败的回执

- **WHEN** 存储不可写时用户打开项目或建立分屏
- **THEN** 修改在内存中生效，回执为 `Applied`、`durable=false` 并携带存储错误
- **AND** 客户端把该修改显示为未落盘
- **AND** 存储恢复后的下一次成功写入清除存储错误，同一 revision 的快照不再携带它

#### Scenario: 元数据变更的回执不承诺落盘

- **WHEN** 用户调整分屏比例或移动焦点
- **THEN** 回执为 `Applied`、`durable=false` 且不携带存储错误
- **AND** 请求方不会把该回执当作已落盘确认

#### Scenario: 格式版本高于当前版本

- **WHEN** 工作区文件声明的格式版本高于当前二进制支持的版本
- **THEN** 常驻进程拒绝加载并给出“升级 crossh”的可执行提示，以非零状态退出
- **AND** 该文件逐字节不变，端口文件未被发布
- **AND** 不会出现以空工作区覆盖新版文件的后续写入

#### Scenario: 格式过新时状态命令报告原因

- **WHEN** 工作区文件的格式版本高于当前二进制，常驻进程因此没有发布端口文件，且没有可达且兼容的常驻进程
- **THEN** `crossh daemon status` 以非零状态退出
- **AND** 输出 `not running:` 加一行升级原因，可直接指向该工作区文件
- **AND** 工作区文件逐字节不变

### Requirement: GUI 重启恢复完整布局

GUI 重启后 MUST 恢复完整工作区布局，包括项目、tab 顺序、分屏树、比例、焦点与活跃项目/标签/pane。对仍在运行的会话，GUI MUST 重新接入原会话，MUST NOT 新建 shell 取代它。

#### Scenario: 分屏与不同 cwd 原样恢复

- **WHEN** 一个项目里有二分屏，两个 pane 位于不同 cwd，用户关闭 GUI 后重新打开
- **THEN** 仍然只有一个项目
- **AND** 分屏结构、比例、焦点与两个 pane 的 cwd 与关闭前一致
- **AND** 两个 pane 重新接入各自仍在运行的会话

#### Scenario: 活跃位置恢复

- **WHEN** 用户关闭 GUI 前处在第二个 tab 的右侧 pane
- **THEN** 重新打开后仍然激活第二个 tab 与该右侧 pane

### Requirement: 常驻进程重启后布局立即可读、会话按 attach 物化

常驻进程重启后 MUST 立即从持久化工作区恢复布局，MUST NOT 在启动阶段创建会话。首个客户端 attach 后，常驻进程 MUST 为活跃 tab 中每个**可物化**的 pane 在持久化 cwd 与尺寸上创建会话；可物化指该 pane 无会话、所属项目可用且 pane cwd 存在。非活跃 tab MUST 在其被激活且已 attach 客户端数大于 0 时按同一规则物化。已 attach 客户端数大于 0 时，打开项目、新建 tab、分屏、激活项目、激活 tab 与显式确保会话 MUST 立即物化其新建或新激活的可物化 pane；数量为 0 时 MUST 只修改布局并保持休眠。CLI 打开项目 MUST NOT 单独触发物化。

#### Scenario: 重启后没有客户端接入

- **WHEN** 常驻进程重启完成且尚无客户端 attach
- **THEN** 会话列表为空，不出现任何新 shell
- **AND** 常驻进程仍报告重启前的项目、tab 与 pane 数量与结构

#### Scenario: 首个 attach 物化活跃 tab

- **WHEN** 重启后第一个客户端 attach
- **THEN** 活跃 tab 的每个可物化 pane 得到一个运行在各自最后 cwd 的新会话
- **AND** 非活跃 tab 的 pane 仍然没有会话
- **AND** 不会因为旧会话消失而删除 pane 或 tab

#### Scenario: 只物化可物化的 pane

- **WHEN** 活跃 tab 同时包含已运行、休眠与不可用（项目根或 cwd 缺失）的 pane，客户端 attach
- **THEN** 只有休眠且可物化的 pane 被创建会话
- **AND** 已运行的 pane 不重复创建会话，不可用的 pane 不创建会话

#### Scenario: GUI 已 attach 时打开项目

- **WHEN** 已有客户端 attach，用户用 CLI 打开一个项目（包括需要补 tab 或根目录缺失的项目）
- **THEN** 新项目的新建 pane 在可物化时立即得到会话，不停留在休眠
- **AND** 根目录缺失时该 pane 保持不可用且不创建会话

#### Scenario: 无客户端时 CLI 打开项目

- **WHEN** 没有任何客户端 attach，用户用 CLI 打开一个项目
- **THEN** 项目与 pane 被创建并持久化，pane 保持休眠
- **AND** 之后 GUI attach 时才创建会话

#### Scenario: 后台 tab 延迟创建

- **WHEN** 用户切换到重启前非活跃的 tab
- **THEN** 该 tab 的每个可物化 pane 此时才在最后 cwd 创建会话
- **AND** 已运行或不可用的 pane 不重复创建会话

### Requirement: pane 与 session 生命周期分离

pane 的身份 MUST 在常驻进程重启与 session 重建后保持稳定。会话因 shell 退出而结束时，常驻进程 MUST 关闭对应 pane 并折叠其父分屏；tab 没有 pane 时 MUST 被删除。常驻进程自身停止时 MUST NOT 因为会话结束而修改持久化布局。已解绑会话的迟到退出通知 MUST 被忽略，且 MUST NOT 重复删除 pane。

#### Scenario: 分屏中的一个 shell 退出

- **WHEN** 二分屏中的一个 shell 退出
- **THEN** 该 pane 从布局中移除，兄弟 pane 占据原空间
- **AND** tab 仍然存在，更新后的布局被持久化

#### Scenario: 关闭一个分屏 pane

- **WHEN** 用户关闭二分屏中的右侧 pane
- **THEN** 该 pane 的会话结束，兄弟 pane 占据原空间
- **AND** tab 与其余 pane 的会话继续存在

#### Scenario: tab 的最后一个 shell 退出

- **WHEN** tab 中最后一个 pane 的 shell 退出
- **THEN** 该 tab 被删除
- **AND** 项目仍然存在，可以新建 tab

#### Scenario: 停止常驻进程不删除布局

- **WHEN** 用户显式停止常驻进程
- **THEN** 其中的会话结束
- **AND** 持久化布局保持不变，下次启动按布局重建

### Requirement: 关闭窗口只断开客户端

关闭 GUI 窗口或退出 GUI MUST 只断开客户端连接。它 MUST NOT 结束会话、删除 pane、删除 tab 或修改布局。

#### Scenario: 长命令继续运行

- **WHEN** 某个 pane 中有长命令正在运行，用户关闭 GUI 窗口
- **THEN** 该会话与命令继续运行
- **AND** 重新打开 GUI 后重新接入同一会话并看到后续输出

#### Scenario: 关闭窗口后布局不变

- **WHEN** 用户关闭 GUI 窗口后再次打开
- **THEN** 项目、tab、分屏、比例与焦点与关闭前一致

### Requirement: 项目生命周期操作语义明确

停止项目 MUST 结束该项目下的所有会话并移除其 tab，但保留项目条目。忘记项目 MUST 移除项目条目及其全部布局与会话。这两种操作与关闭 tab 的回执 MUST NOT 等待会话结束；回执返回时快照 MUST 已不含目标 pane，会话结束 MAY 稍后完成，迟到的会话退出通知 MUST NOT 再次修改布局。操作结果 MUST 对所有已连接客户端一致。

#### Scenario: 停止项目

- **WHEN** 用户停止一个项目
- **THEN** 该项目下所有会话结束，tab 被移除
- **AND** 项目仍出现在项目列表中，可以重新打开新 tab

#### Scenario: 忘记项目

- **WHEN** 用户忘记一个项目
- **THEN** 该项目及其全部 tab、pane 与 session 从所有客户端消失
- **AND** 常驻进程不再为它保留布局

### Requirement: 布局修改有回执与可判定的 revision

每个工作区修改 MUST 返回回执，包含请求标识、结果、当前 revision、本次新建的标识、`durable` 与可选的存储错误。结果 MUST 是 `Applied`、`Unchanged`、`Conflict` 或 `Rejected` 之一。revision MUST 只在持久布局字段（项目、tab、pane、分屏、顺序、名称、cwd、尺寸、焦点与活跃位置）实际变化时增加 1；仅运行时字段（可用性、会话绑定、cwd 可用性、pane 状态与存储错误）变化的操作 MUST 返回 `Applied`、保持 revision 并重发快照；有效但无变化 MUST 返回 `Unchanged`；冲突与拒绝 MUST NOT 改变 revision。请求 MAY 携带 `expected_revision`，不匹配 MUST 返回 `Conflict`；未携带时按最后写入者生效。回执与随后的快照 MUST 携带同一 revision，客户端 MUST 合并同 revision 快照的运行时字段。等待回执超时 MUST 视为失败。

#### Scenario: 新建对象返回稳定标识

- **WHEN** 客户端请求在某个 pane 上建立分屏
- **THEN** 回执为 `Applied`，携带新的分屏与 pane 标识以及增加后的 revision
- **AND** 其他客户端收到的快照包含同一批标识

#### Scenario: no-op 修改不增加 revision

- **WHEN** 客户端激活一个已经活跃的项目，或用相同比例设置分屏
- **THEN** 回执为 `Unchanged`，revision 与操作前相同
- **AND** 调用方不会因为等待 revision 前进而超时

#### Scenario: 并发修改按 CAS 判定

- **WHEN** 两个客户端都携带同一个 `expected_revision` 修改同一 tab
- **THEN** 先到者得到 `Applied`，后到者得到 `Conflict` 与当前 revision
- **AND** 后到者可以按新 revision 重试

#### Scenario: 修改被拒绝

- **WHEN** 客户端请求修改一个 live pane 的 cwd
- **THEN** 回执为 `Rejected` 并给出可判定的错误码
- **AND** revision 与布局均不变化

#### Scenario: 显式物化不推进 revision

- **WHEN** 客户端在已有客户端 attach 时请求确保一个休眠且可物化的 pane 创建会话
- **THEN** 回执为 `Applied`，revision 与操作前相同
- **AND** 快照中该 pane 的状态转为正在启动，布局字段不变
- **AND** 其他客户端收到的同 revision 快照只在该运行时字段上不同

#### Scenario: 已运行或不可用的 pane 不被重新物化

- **WHEN** 客户端请求确保一个已运行、正在启动或不可用的 pane
- **THEN** 回执为 `Unchanged`，revision 不变
- **AND** 不创建新会话

#### Scenario: 可用性刷新只改运行时字段

- **WHEN** 项目根目录的状态发生变化，客户端请求刷新可用性
- **THEN** 回执为 `Applied`，revision 与操作前相同
- **AND** 快照只更新可用性与受影响的 pane 状态，布局字段保持不变

#### Scenario: 可用性没有变化

- **WHEN** 客户端请求刷新可用性且没有任何运行时字段变化
- **THEN** 回执为 `Unchanged`，revision 不变

### Requirement: 布局操作的目标与默认值明确

`OpenProject` MUST 激活项目并保证至少一个 tab：已存在时补齐缺失的 tab，新项目插到最前并建一个 tab 与单 pane（cwd 为项目根）；根缺失时仍建该 tab 与 pane，但项目与 pane 不可用。`ActivateProject` MUST 只切换活跃项目，MUST NOT 创建 tab；迁移来的空项目被激活后保持 0 个 tab。`CreateTab` 未指定项目时 MUST 用活跃项目，没有活跃项目 MUST 被拒绝；项目内没有 pane 时初始 cwd MUST 为项目根，否则取活跃 pane 的 cwd。分屏 MUST 指定目标 pane 与轴向，未指定 cwd 时 MUST 继承目标 pane 的 cwd；关闭 tab MUST 关闭其全部 pane 并激活相邻 tab；修改 pane cwd MUST 只对休眠或不可用的 pane 生效，其余 MUST 被拒绝；tab 重排 MUST 以“排到某个 tab 之前”表达，未指定目标时排到末尾。项目顺序 MUST 由持久化顺序决定；打开已有项目移到最前，新项目插到最前。

#### Scenario: 激活空项目不创建 tab

- **WHEN** 用户激活一个迁移而来、没有任何 tab 的项目
- **THEN** 该项目成为活跃项目且仍有 0 个 tab
- **AND** 没有新建 pane 或会话
- **AND** 之后新建 tab 时初始 cwd 为项目根

#### Scenario: 打开根缺失的新项目

- **WHEN** 用户打开一个不存在的项目根目录
- **THEN** 项目被创建、插到最前并带一个 tab 与单 pane
- **AND** 项目与 pane 都报告为不可用，原因说明根目录缺失
- **AND** 目录恢复后无需重建布局即可物化该 pane

#### Scenario: 在空项目里新建 tab

- **WHEN** 迁移来的项目没有任何 tab，用户在它被激活后新建 tab
- **THEN** tab 建在该项目内，初始 cwd 为项目根
- **AND** 新 pane 随即被物化

#### Scenario: 分屏继承 cwd

- **WHEN** 用户在一个 cwd 为 `/repo/crates/a` 的 pane 上建立左右分屏且未指定 cwd
- **THEN** 新 pane 的 cwd 为 `/repo/crates/a`
- **AND** 两个 pane 属于同一 tab、同一项目

#### Scenario: 关闭多分屏 tab

- **WHEN** 用户关闭一个含三个 pane 的 tab
- **THEN** 三个 pane 的会话全部结束
- **AND** 相邻 tab 被激活

#### Scenario: 项目顺序

- **WHEN** 用户重新打开一个已在列表中的项目
- **THEN** 该项目移到项目顺序最前
- **AND** 其他项目的相对顺序不变

### Requirement: pane 运行状态与错误可观察

工作区快照 MUST 暴露每个项目的可用状态、每个 pane 的会话绑定、cwd 可用性、运行状态（正在启动、已运行、休眠并携带原因、不可用并携带原因）与工作区的存储错误。原因 MUST 使用稳定错误码：休眠原因为 `not_started` 或 `spawn_failed`，不可用原因为 `project_root_missing` 或 `cwd_missing`。运行状态 MUST 先看会话绑定：已绑定 → 已运行；否则进行中 → 正在启动；否则项目可用且 cwd 存在 → 休眠；否则不可用（项目根缺失或 cwd 缺失）。已绑定会话时 MUST 仍暴露会话标识，项目根或 cwd 缺失 MUST NOT 把该 pane 报成不可用；可用性与 cwd 可用性 MUST 独立暴露。物化失败 MUST 让 pane 保持休眠并记录原因。cwd 缺失 MUST 保留原值并标为 cwd 不可用。无会话且不可用时 MUST NOT 创建会话；已有会话 MUST NOT 因目录缺失被结束。

#### Scenario: 会话创建失败

- **WHEN** 某个 pane 的 shell 创建失败
- **THEN** 该 pane 在快照中显示为休眠并携带失败原因
- **AND** 其余 pane 与 tab 不受影响
- **AND** 客户端可以请求重新物化该 pane

#### Scenario: pane cwd 不存在

- **WHEN** 某个 dormant pane 的 cwd 已被删除
- **THEN** 快照仍然报告该 cwd，并把 pane 标为不可用、原因为 cwd 缺失
- **AND** 用户可以把 cwd 改成存在的目录后重新物化
- **AND** 在 cwd 恢复或修改之前不会为该 pane 创建会话

#### Scenario: 项目根缺失时无会话 pane 不可用

- **WHEN** 某个项目的根目录已被删除，而项目内 pane 的 cwd 仍然存在
- **THEN** 该项目内所有没有会话的 pane 都报告为不可用，原因为项目根缺失
- **AND** 不会尝试为它们创建会话
- **AND** 项目根恢复后它们按 cwd 状态回到休眠或已运行

#### Scenario: live pane 的 cwd 被删除

- **WHEN** 某个已运行 pane 的 cwd 目录被删除
- **THEN** 快照仍然给出该 pane 的会话标识，运行状态仍为已运行
- **AND** cwd 可用性单独报告为不可用，cwd 原值保留
- **AND** 客户端可以继续接入该会话，不会因为目录缺失而新建 shell 或报告 pane 不可用

#### Scenario: live pane 的项目根被删除

- **WHEN** 某个项目的根目录被删除，而项目内某些 pane 仍有运行中的会话
- **THEN** 该项目标记为不可用，但有会话的 pane 保持已运行并携带会话标识
- **AND** 客户端可以重新接入这些会话
- **AND** 项目根恢复后无需重新物化即可报告为可用，且不会重复创建会话

### Requirement: 旧设置一次性迁移

常驻进程 MUST 在开始服务之前把旧设置中的固定标签与最近目录导入工作区；失败时 MUST 以未迁移状态继续服务并在下次启动重试。导入 MUST NOT 依赖 GUI 触发，也 MUST NOT 以“工作区为空”为前提。固定标签 MUST 导入为项目下的 tab 与单个 pane，保留 `pin_id`、`project_dir`、`cwd`、`custom_name`、`default_command` 并丢弃 `daemon_session_id`；cwd 缺失时取项目根。最近目录 MUST 导入为没有 tab 的空项目并按根目录去重。固定标签 MUST 排在最近目录之前；活跃项目取第一个固定标签项目，没有时取第一个最近目录。缺失的项目根 MUST 保留并标记不可用。导入完成后 MUST 记录一次性标记并持久化；标记写入失败时 MUST NOT 记录成功，下次启动 MUST 重试。迁移完成前 MUST NOT 改写旧设置；完成后的保存路径与旧值保留由「设置路径的旧版本只读回退」定义。

#### Scenario: 固定标签迁移为普通 tab

- **WHEN** 旧设置中有一个固定标签，带有 cwd、自定义名称与默认命令
- **THEN** 首次启动后它成为一个项目下的普通 tab
- **AND** cwd、名称与默认命令被保留，标签仍然可编辑
- **AND** 旧会话标识被丢弃，pane 在 attach 后得到新会话

#### Scenario: 只导入一次

- **WHEN** 用户完成迁移后关闭所有项目并重启
- **THEN** 旧设置中的条目不会再次出现
- **AND** 工作区中的迁移标记阻止重复导入

#### Scenario: CLI 先打开项目

- **WHEN** 新版本首次运行就是 `crossh open /repo`，工作区在无 GUI 时已非空
- **THEN** 旧设置中的固定标签与最近目录仍然被导入
- **AND** 与 `/repo` 相同根目录的条目被合并而不是重复

#### Scenario: 迁移写盘失败后重试

- **WHEN** 首次迁移时工作区文件写入失败
- **THEN** 常驻进程不记录迁移完成
- **AND** 下次启动重新执行导入，不产生重复项目或 tab

### Requirement: 设置路径的旧版本只读回退

当 `config_root()/settings.toml` 不存在、未设置 `CROSSH_CONFIG_DIR` 且旧版路径 `~/.config/crossh/settings.toml` 存在时，常驻进程与客户端 MUST 只读该旧文件读取设置，新写入 MUST 落在 `config_root()`。迁移读取 MUST NOT 在迁移完成前改写源文件；debug 回退源 MUST NOT 被新 debug 写入（保持逐字节不变）；release 的 `settings_write` 与迁移源同路径，首次保存 MAY 重写源文件，重写后 MUST NOT 再包含 `recent_local_dirs` 与 `pinned_local_tabs`，旧值 MUST 由源文件旁的 `.pre-workspace` 快照保留。只读快照 MUST 写在实际读取的源文件同目录。debug 与 release 的迁移标记各自独立，同一旧文件 MUST 可被两者各迁移一次。`config_root()/settings.toml` 出现后 MUST NOT 再回退。

#### Scenario: 旧 debug 设置迁移到新 debug 根

- **WHEN** 旧 debug 构建只在 `~/.config/crossh/settings.toml` 留下最近目录与固定标签，新版 debug 首次启动且 `~/.config/crossh-dev/settings.toml` 不存在
- **THEN** 新版读取旧文件并把它导入 `crossh-dev` 工作区
- **AND** 只读快照写在 `~/.config/crossh/settings.toml.pre-workspace`
- **AND** 旧文件逐字节不变（新版不写入该文件）
- **AND** 新版写入落在 `~/.config/crossh-dev/settings.toml`，该文件出现后不再读取旧文件

#### Scenario: release 首次保存重写旧文件

- **WHEN** release 首次启动完成迁移后，用户修改任一设置并保存
- **THEN** 保存写入与迁移源相同的 `~/.config/crossh/settings.toml`
- **AND** 重写后的文件不再包含旧版的最近目录与固定标签字段
- **AND** 旧值仍可从 `~/.config/crossh/settings.toml.pre-workspace` 恢复

#### Scenario: 新根出现后停止回退

- **WHEN** `config_root()/settings.toml` 已存在，旧路径文件也仍在
- **THEN** 设置读取只使用新根文件，旧文件不再参与
- **AND** 保存 settings 只写新根文件

### Requirement: 协议版本与能力不匹配时明确拒绝

工作区协议版本不一致 MUST 在握手阶段被拒绝，并 MUST 给出可执行的恢复步骤。客户端发现常驻进程存活但版本不兼容时 MUST NOT 删除其端口文件，MUST NOT 启动第二个常驻进程。`crossh daemon stop` MUST 能停止当前版本与上一版本（v1）的常驻进程，且 MUST NOT 在停止失败或版本不兼容时拉起新实例。`crossh daemon status` MUST 区分三种情况：没有端口文件或端口不可连接时报告未运行；端口可达但握手报版本不兼容时报告版本不兼容，给出两侧版本与 `crossh daemon stop`，并以非零状态退出；端口可达且握手成功时报告运行状态。该命令 MUST 使用无副作用探测，MUST NOT 删除端口文件，MUST NOT 启动常驻进程。`daemon.error` 的内容 MUST 只在没有可达且兼容的常驻进程时展示。客户端与常驻进程共同支持 `workspace-v1` 时才能使用工作区；未声明该能力时，客户端 MUST 拒绝连接并提示升级，MUST NOT 静默降级；客户端不认识的能力标志 MUST 被忽略。

#### Scenario: 新客户端连接旧常驻进程

- **WHEN** 新客户端连到旧协议版本（v1）的常驻进程
- **THEN** 握手在交换工作区数据之前失败
- **AND** 失败原因说明是版本问题，并给出停止旧实例的具体命令
- **AND** 系统中仍然只有一个常驻进程，端口文件仍由旧实例持有

#### Scenario: 停止旧版本常驻进程

- **WHEN** 用户在新版本二进制上执行 `crossh daemon stop`，而运行中的是 v1 常驻进程
- **THEN** 命令以 v1 握手完成认证并发送停止请求
- **AND** 旧常驻进程退出，端口文件由它删除，命令以 0 退出
- **AND** 该命令不启动新的常驻进程

#### Scenario: 状态命令识别存活的旧常驻进程

- **WHEN** v1 常驻进程正在运行，用户在新版二进制上执行 `crossh daemon status`
- **THEN** 输出 `incompatible:` 加两侧版本与 `crossh daemon stop`，以非零状态退出
- **AND** 不把该实例报告为陈旧端口文件或未运行

#### Scenario: 状态命令不产生副作用

- **WHEN** v1 常驻进程正在运行，用户在新版二进制上执行 `crossh daemon status`
- **THEN** 命令以非零状态退出并报告版本不兼容
- **AND** 系统中仍然只有一个常驻进程，端口文件仍由旧实例持有且未被删除

#### Scenario: 陈旧错误文件不掩盖运行状态

- **WHEN** `daemon.error` 残留，而端口可达且握手成功
- **THEN** `crossh daemon status` 输出运行状态与会话数量
- **AND** 不展示陈旧错误内容

#### Scenario: 缺少工作区能力

- **WHEN** 常驻进程握手成功但没有声明 `workspace-v1`
- **THEN** 客户端拒绝把它当作工作区属主并提示升级
- **AND** 不会出现 GUI 自己持有布局、daemon 只持会话的混合状态

### Requirement: 缺失项目根时保留布局并可按需刷新

项目根目录不存在时，常驻进程 MUST 保留项目及其布局，并 MUST 将该项目的不可用状态暴露给客户端。它 MUST NOT 自动删除项目、把 pane 移到其他项目或静默改写项目根。已有会话的 pane MUST 保持已运行并继续可接入，目录缺失 MUST NOT 结束会话。可用性 MUST 在读取快照、客户端 attach、激活项目以及窗口重新获得焦点时的显式刷新中重新计算；目录恢复后 MUST 无需任何布局修改即可报告为可用。

#### Scenario: 项目目录被重命名

- **WHEN** 常驻进程启动时某个项目的根目录不存在
- **THEN** 该项目仍出现在工作区中并标记为不可用
- **AND** 它的 tab、pane、比例与焦点保持不变
- **AND** 其他项目不受影响

#### Scenario: 目录恢复后重新可用

- **WHEN** 缺失的项目根目录被恢复，客户端重新读取、attach 或显式刷新工作区
- **THEN** 该项目恢复为可用状态
- **AND** 没有会话的 pane 可以重新创建会话，已有会话的 pane 不重复创建

### Requirement: 任意前端可以读写工作区

不依赖 GUI 的前端 MUST 能打开或激活项目、读取工作区快照，并看到项目、tab、pane 与 session 的归属关系。CLI 打开项目 MUST 在收到 `Applied` 或 `Unchanged` 回执后才报告成功；拒绝、超时或版本不兼容 MUST 以非零状态退出且 MUST NOT 启动 GUI。会话列表 MUST 同时显示每个会话的输入权角色与它所属的项目、tab、pane；不属于任何布局的会话 MUST 被明确标出。项目、tab 与 pane 三列 MUST 使用稳定标识而不是可能含空白的显示名，cwd MUST 保持为最后一列，使含空格的路径能按剩余字段解析。常驻进程状态输出 MUST 同时给出会话数量与工作区计数；无法连接时 MUST 区分未运行、陈旧端口文件与存活但不兼容，`daemon.error` 的展示前提遵循「协议版本与能力不匹配时明确拒绝」。

#### Scenario: GUI 从未运行

- **WHEN** 用户在没有任何 GUI 进程运行的情况下用 CLI 打开一个目录
- **THEN** 常驻进程创建或激活对应项目并返回回执
- **AND** 之后启动 GUI 时直接看到该项目的布局

#### Scenario: CLI 等待回执

- **WHEN** `crossh open <path>` 的修改被拒绝或超时
- **THEN** 命令以非零状态退出并说明原因
- **AND** GUI 不会被启动或唤起

#### Scenario: CLI 查看层级

- **WHEN** 用户用 CLI 列出会话
- **THEN** 输出包含每个会话的输入权角色、所属项目、tab 与 pane
- **AND** 不属于任何布局的临时会话在这三列显示占位符
