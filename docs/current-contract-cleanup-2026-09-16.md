# 当前正式契约清理工作记录

> 阶段记录：本文保留实施当时的路径、限制和测试结果，不是当前接口规范。当前职责与入口以[架构说明](architecture.md)和[文档索引](README.md)为准。

## 基线与范围

本轮接手 `d4e22a0cc76f82d49afd33456002a32098ab9e04` 之上的未提交工作区。
已有产品改名、质量整理及更早的 README 和发布风险文档修改全部保留。
此前结果见 [代码质量报告](code-quality-review-2026-09-16.md)，不得将该报告的通过数视为本轮修改后的验收结果。

本轮按用户的新政策清理仅服务旧密钥机制和旧工具调用的兼容代码，不合入另一 checkout 中尚未提交的流式导出、语音字段和构建来源新功能。
不删除用户磁盘上的任何旧配置、密钥或数据库；不自动提交、推送或发布。

## 当前实际修改

- 普通配置读取与初始化配置读取共用 `config::validate_key_configuration`，明确拒绝 `image_aes_key`、`image_xor_key`，包括值为 null 的旧字段。错误不包含密钥值，检查不改写配置。
- 已明确绑定账号的初始化不再通过旧逐库文件或旧账号密钥文件是否存在来选择存储机制。缺省只配置 `keys.dpapi`，不读取或导入旧文件。
- 保留未绑定账号却存在来源未知密钥文件时的拒绝。这是账号归属保护，不是旧格式解析或自动迁移。
- 保留正式 DPAPI 存储使用的现有绑定锚点；删除读取链路不等于允许改写正式存储的账号身份。`keys_file` 当前仍参与这一身份及输出保护，不应被误称为仍在读取逐库明文。

新增合成反例覆盖：正式存储与旧明文字段同时存在仍拒绝、错误脱敏、拒绝后文件不变，以及旧文件内容损坏也不会被初始化读取或删除。
旧迁移入口、旧发布入口已完成源码及第一方调用方清理；对应根工程与独立夹具的最终整体验收仍未完成。

## 移除的兼容机制

| 已删除或拒绝 | 当前唯一方式 | 已同步范围与用户影响 |
| --- | --- | --- |
| `extract` 默认图片 provider、隐式扫描和全局配置重读 | 同一 daemon QueryLease 的图片材料与受保护发布 | 服务路由、提取实现、缺钥诊断与合成测试同步。缺 V2 材料需先显式授权初始化图片密钥；已有正式存储仍可用，不删除任何用户文件。 |
| 一键准备缺钥后的隐式扫描及直接保存 | daemon 只读数据库快照；缺钥先显式初始化 | 正式 Decrypt、ExportEmoticons 和按需的 ExportAll 已接线；`Prepared*` 过渡操作已删除。缺钥测试改为不写入拒绝，路径/HMAC/旧文件保护保留。既有材料仍可用；不删除磁盘文件。 |
| `wx-toolbox` 二进制、旧启动器及自动转发 | 正式 `wx` CLI，经既有 daemon 执行 | Cargo、主入口、安装器、发布流程和 npm 测试已更新。旧工具调用必须改为正式入口，不保留外壳。 |
| 旧 npm 包目录、旧下载附件及相关安装回退 | `wx-workbench` 包和当前 Windows x64 MSVC 附件 | npm、发布与安装脚本同步；使用旧包的用户需按当前说明安装。第三方署名与历史报告不修改。 |
| `migrate-keys`、逐库明文 JSON 导入、旧账号单文件读取 | 当前按账号绑定的 DPAPI `key_store` | CLI、操作契约、执行模块与夹具种子同步。旧文件不再自动读取或迁移，需要重新初始化；不删除磁盘旧文件。 |
| 配置内 `image_aes_key` / `image_xor_key` | 正式存储中的已验证图片材料 | 通用配置及初始化读取一致拒绝，含 null 值。用户需移除旧配置字段并按正式入口重新准备材料；错误不包含秘密，配置不被自动改写。 |
| 仅保存一个 XOR 字节的旧图片材料记录 | 完整 AES 与 XOR 材料的类型化记录 | 存储测试、图片及音频夹具同步。旧加密记录长度被明确拒绝；不删除实际 XOR 解码及样本探测能力。 |
| 联系人 `legacy_view` 分流与独立旧生产实现 | CLI、HTTP、MCP 共用正式联系人业务 | 请求、MCP schema、共享查询、夹具和联系人文档同步。旧参数任意取值均拒绝，正式返回为 `contacts/total` 及 `username/display`。 |

当前仍保留有正式用途的 `wx` 命令、`WX_CLI_CONFIG` 等账号配置入口，以及正式运行身份/持久化标记；不能仅凭旧前缀将它们作为无用别名删除。`keys_file` 仍用于身份锚定和输出保护，不再表示恢复明文读取链路。各项回归证据与未覆盖范围见下文，未把文本扫描当作运行验证。

## 复核但未重复改写

`toolkit/monitor.rs` 在输出批次成功后才更新内存游标；`incomplete` 或 `truncated` 为真时保持原游标。已有这项行为，本轮不为降低函数长度重新改写状态转移。
历史复杂度指标及上一轮提取责任的修改保留在原质量报告，不声称归零。本轮额外运行 `cargo clippy --offline --locked --target x86_64-pc-windows-msvc --bin wx -- -W clippy::cognitive_complexity`，与默认严格 Clippy 分开记录。

| 生产函数 | 接手报告记录 | 当前实测 |
| --- | ---: | ---: |
| daemon `dispatch` | 52 | 50 |
| `run_monitor` | 30 | 30 |
| `run_latency` | 31 | 31 |
| `write_export_with_publication` | 26 | 26 |

接手列是历史报告值，不是重新检出旧代码实测。第一次本轮测量还发现前台 `execute` 因重复刷新判断达到 26；修正已有操作分类为 `requires_snapshot_reload`、排除已自行发布快照的数据库取钥后，宿主删除了第二份判断，第二次测量不再超过阈值 25。没有为指标添加转发函数或关闭规则。完整日志分别为 `C:/CodexLocal/wx-workbench-worker-complexity-1.log`、`C:/CodexLocal/wx-workbench-worker-complexity-2.log`。上述 4 个热点仍保留：完整路由、监测状态转移与发布次序有实际可读性价值，后续迁移按责任而不是分数推进。

## 验证进度

后续阶段的解析资源迁移、密钥材料清零、监控恢复及新一轮根工程全量检查见[架构优化实施与验收](architecture-optimization-2026-09-16.md)。该轮全量唯一失败为测试 CLI Job 与 daemon 清理的生命周期冲突，修复后原四项安全测试复测通过；以下保留各历史检查原始结果。

使用同一 checkout 专属的 `C:/CodexLocal/wx-workbench-target-20260916`，不与 `C:/CodexLocal/src/wx-cli` 共用构建输出。

- 修改前状态：上一任务报告严格 all-target Clippy 无诊断，根测试 2882 次通过、22 次忽略；这是接手记录，不是本轮复跑。
- 初次 `cargo check --offline --locked --target x86_64-pc-windows-msvc` 未通过：编译发生于旧模块删除与引用清理之间，出现缺失模块及未使用导入。日志 `C:/CodexLocal/wx-workbench-cleanup-check-1.log`。不是 libclang 环境阻断，也未计为通过。
- 中途 `scripts/check-quality.ps1 -Stage Check -Offline` 已通过全目标编译和严格 all-target Clippy，退出码 0。该轮未观察到源码变化，但不是最终验收。完整日志目录为 `C:/CodexLocal/wx-workbench-target-20260916/quality-checks/20260916T002849895Z-aa7f2d96c2d3403ca167ec898a35718a`。
- 中途根工程 all-target 全量测试已完成，采用 `--no-fail-fast -- --test-threads=1`：31 个目标，2881 次通过、13 次失败、23 次忽略，退出码 1。检查期间源码有变化，明确标记为中间状态。日志目录为 `C:/CodexLocal/wx-workbench-target-20260916/quality-checks/20260916T002936404Z-51e476993109418badcd96cc0c863ca0`。
- 失败分为：恢复测试重复建表；新 CLI 测试错误拒绝本 checkout 独占的外置 target；旧迁移命令错误消息断言；夹具仍假定准备过程会创建 daemon 运行目录；语音夹具仍向正式种子接口提供旧形状。分别修复中，尚未将复测计为通过。账号隔离、参数拒绝与不改写文件的断言不取消。
- 随后当前生产目标 `cargo check --offline --locked --bin wx --target x86_64-pc-windows-msvc` 已通过且无告警，见 `C:/CodexLocal/wx-workbench-cleanup-check-4.log`。这覆盖联系人分流清理和服务端 pipe peer PID 接线，但不代替运行验证。独立 fixture、最终全量与复杂度检查仍待完成。
- 失败后的定向复测：密钥快照相关 13 项通过、0 失败/忽略（`C:/CodexLocal/wx-workbench-query-keys-retest-2.log`）；服务管道相关 12 项通过、0 失败/忽略（`C:/CodexLocal/wx-workbench-transport-retest-1.log`）。首个定向编译曾发现两个新接口的夹具残留，修复后通过，未恢复旧接口。
- 5 个失败集成目标的复测共 42 次通过、0 失败、3 次忽略（`C:/CodexLocal/wx-workbench-failed-targets-retest-1.log`），覆盖 `current_entry_contract`、`retired_key_migration_cli`、`delta_plan_security`、`runtime_isolation`、`voice_runtime`。原中途全量中的 13 个失败已有对应修复复测；这些不是新的整仓最终全量结果。
- 上述集成复测编译时，正在开发的密钥更新事务接口尚无生产调用，产生 4 条未使用告警（`KeyChange`、借用转换、发布保护对象、更新方法）。后续须通过真正接线消除，不添加 `allow/expect`。因此不能沿用前一检查轮的零告警结论宣称当前源码最终无告警。

本轮尚未完成验收。没有访问真实微信账号、扫描进程、读取真实密钥或调用外部转录服务。

### worker 写入通道阶段检查

- 真实前台操作回归 `cargo test --offline --locked --target x86_64-pc-windows-msvc --test entry_service_runtime -- --test-threads=1`：4 通过、0 失败/忽略。验证本 checkout 的 worker 标准输出、退出码、相对路径、内部环境拒绝、取消及租约超时的进程树回收。日志 `C:/CodexLocal/wx-workbench-worker-entry-test-1.log`；当时一个失效 `Path` 导入告警随后已删除。
- `cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx worker_keys -- --test-threads=1`：13 通过、0 失败、1 忽略。该忽略项是由测试显式启动的合成 Job 子进程入口，不是跳过验收。真实命名管道、实际 peer PID、私有 stdin、人工数据库材料及 DPAPI 保存贯通；覆盖越权、撤销、退出、并发 revision 冲突和乱序映射幂等重试。日志 `C:/CodexLocal/wx-workbench-worker-keys-test-1.log`。仍不触发真实取钥算法。
- `scripts/check-quality.ps1 -Stage Check -Offline`：全目标编译和严格 all-target Clippy 均退出 0，未观察到源码变化；日志目录 `C:/CodexLocal/wx-workbench-target-20260916/quality-checks/20260916T012701740Z-9efeb5dc278b4946a235686bc1b15f35`。这是默认规则的阶段检查，不包括最终全量测试、独立 fixture 矩阵或额外复杂度规则。
- 补充等待取消和父进程创建时间不匹配反例后，同一 `worker_keys` 测试命令复跑为 14 通过、0 失败、1 子进程入口忽略；日志 `C:/CodexLocal/wx-workbench-worker-keys-test-2.log`。限时观察 grant 锁确认提交已接收后取消等待，仍完成持久化及幂等记录；真实管道的错误父代次请求在 handler 收到任何调用前即被拒绝。
- `cargo fmt --all -- --check` 首次发现本轮前序修改遗留的排版差异，仅对报告指出的 11 个已修改文件格式化，复跑退出 0；日志 `C:/CodexLocal/wx-workbench-worker-format-2.log`。随后严格 all-target Clippy 再次退出 0，见 `C:/CodexLocal/wx-workbench-worker-clippy-2.log`。额外复杂度告警并不等于默认告警未通过。
- 最后执行主程序完整单元集 `cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx -- --test-threads=1`：1392 通过、0 失败、11 忽略，耗时 248.79 秒，日志 `C:/CodexLocal/wx-workbench-worker-bin-test-1.log`。覆盖最终刷新分类与等待取消反例。11 个忽略项中 6 个是父测试启动的合成子进程入口；其余 5 个为显式原生 Frida、3 个 FFmpeg 音频/表情集成及符号链接权限测试，本轮未启用，也未计为通过。主程序单元集不替代根工程全部集成目标和独立 fixture 矩阵。
- 最后一次 `scripts/check-quality.ps1 -Stage Check -Offline` 全目标编译和严格 Clippy 均退出 0，未观察到源码变化；目录 `C:/CodexLocal/wx-workbench-target-20260916/quality-checks/20260916T014017099Z-903aac179c944e358247cabe153170d8`。最终阶段格式复查亦退出 0，日志 `C:/CodexLocal/wx-workbench-worker-format-3.log`。没有提交或推送。

旧 `src/daemon/query/mcp_contacts_legacy` 源文件已删除，剩余空目录的删除被执行环境策略拦截，未绕过限制；它不进入 Git 提交，也不影响构建。未删除任何真实用户文件。

### 持久任务数据库取钥接线

随后复核图片调用方时发现，持久任务 `Step::WechatKeys` 同样调用已迁移的数据库取钥函数，但当时任务 worker 没有安装私有凭据，会在扫描前拒绝。此前前台阶段验证没有覆盖该调用路径，这是实际遗漏，不能用阶段测试通过掩盖。

现已将同一个 daemon `Broker` 注入任务服务：任务进程沿用挂起启动、Job 监督及原私有 stdin，改为 `Input<Step>`，仅对固定配置且明确授权的 `WechatKeys` 步骤签发数据库材料权限。凭据按步骤回收，不写任务历史。任务 worker 输入自动清零并校验帧结束；旧内部裸 Step 帧不保留兼容解析。普通任务步骤不读取密钥以申请无关权限。数据库取钥任务完成后不再重复丢弃 daemon 已发布快照，取消和终态仍由原任务服务负责。

`Step::WechatDecrypt` 现同样通过该 broker 获得 `READ_DATABASES` 只读材料：worker 使用 `service::worker_keys::database_keys` 取得当前快照后，调用既有严格解密准备流程。该访问不包含账户主密钥、图片材料或初始化 seed，也不具备 `WorkerKeys` 写权限；配置路径必须精确匹配同一个 `RuntimeContext`。合成测试通过真实受监督 worker、daemon broker、命名管道和人工数据库材料验证了提交、读取及越权更新拒绝，结果为 28 passed、1 ignored。其他会读取密钥的任务步骤仍待逐项接入，不能宣称 worker 的材料访问已经全部统一。

全目标编译通过且无告警，日志 `C:/CodexLocal/wx-workbench-task-keys-check-1.log`。私有通道定向测试 17 通过、0 失败、1 子进程夹具入口忽略，日志 `C:/CodexLocal/wx-workbench-task-keys-test-1.log`；新增测试经真实管道、受监督子进程及 `Input<Step>` 验证人工数据库材料保存、幂等、错误配置及 Account/Image 越权拒绝。不执行真实取钥，图片取钥尚未迁移。

实际入口复测的首轮在 `entry_architecture` 中发现失效的文本断言：它要求局部变量必须叫 `operation`，因此不接受新的 `request.operation`。该轮为 4 通过、1 失败，尚未运行 `runtime_isolation`，日志 `C:/CodexLocal/wx-workbench-task-runtime-test-1.log`。现已复用原有 syn 生产依赖检查，检查 Rust 路径而非拼写；CLI/客户端禁止本地执行的检查同时覆盖别名及空白变化。新增反例区分真实依赖、注释、字符串、无关函数和显式测试代码，没有删除架构约束。

修正后使用 `--no-fail-fast` 复跑两个目标：入口架构 6 通过、账号/任务运行时 18 通过、0 失败、2 个原有 FFmpeg 条件测试忽略。日志 `C:/CodexLocal/wx-workbench-task-runtime-test-2.log`；实际本 checkout 的 `wx.exe` 验证任务导出、跨 CLI/Web/MCP 操作、断连后幂等重试、取消回收、历史持久化、账号拒绝及重启后 `interrupted`，不宣称自动断点续跑。另跑 `daemon::tasks`：10 通过、0 失败、1 子进程入口忽略，日志 `C:/CodexLocal/wx-workbench-task-service-test-1.log`。

阶段收尾 `cargo fmt --all -- --check` 退出 0，日志 `C:/CodexLocal/wx-workbench-task-format-1.log`；`scripts/check-quality.ps1 -Stage Check -Offline` 全目标编译和严格 Clippy 均退出 0，未观察到源码变化，完整日志目录 `C:/CodexLocal/wx-workbench-target-20260916/quality-checks/20260916T015554156Z-95b4fd7fef8347c6803eb9b6667122a4`。本阶段未重跑全部根集成目标、独立 fixture 矩阵及额外复杂度规则，未将上一阶段结果替作最终整体验收。

## 追加范围：运行期密钥快照

用户追加要求 daemon 是运行期间唯一密钥状态管理者。本项正在实施，不能将已有 `DbCache` 数据库密钥缓存称为已经完成统一管理。

原 `QueryState::snapshot` 在每次请求中加载密钥库取得 revision，而 `initialize` 又加载一次。现已使用独立的惰性密钥 cell：并发首次加载合并，失败允许重试，查询代际复用同一快照；Web 图片解码从查询租约取材料，任务脱敏器由宿主供给快照，不再独立解密密钥库。显式密钥失效按 query、key、cache_work 的顺序等待。计数及生命周期测试已加入，运行结果待全量测试确认。其他媒体及独立 worker 执行路径仍待接线，不能宣称全入口统一。

后续实现须复用现有租约、失效和进程身份校验，覆盖首次并发加载、失败重试、更新持久化与内存切换顺序，并为独立 worker 使用受限内部通道；不能通过普通环境变量、命令参数、公开任务字段或明文临时文件交付密钥。外部文件替换要求显式重新加载或重启，不能继续依赖热路径读文件探测。

服务端已补充从命名管道句柄读取客户端 PID，并传入内部处理函数；上述 12 项真实管道定向测试已验证进程身份读取。私有 worker 更新在此基础上增加受限凭据及原进程句柄校验，不将单独的 PID 读取称为完成认证。worker 客户端另固定父 daemon 的 PID 和创建时间，在发送任何凭据或材料前拒绝进程代次变化。

worker 写入通道已开始接线：前台操作的现有私有 stdin 帧携带受限能力；daemon 使用实际 pipe peer PID、原始子进程的复制句柄、存活注册和材料权限共同校验，不能仅凭公共服务 token 写入。注册随 worker 回收撤销；不使用秘密环境变量、命令参数或明文临时文件，不增加后台服务。

`DatabaseKeys` 已改为验证材料后通过 `service::worker_keys` 提交给 daemon `QueryState::update_keys`，不再自行保存密钥文件。原路径保护、扫描授权和材料验证保留；旧 `snapshot_keys` 文件比较函数失去调用方后删除。更新事务负责持久化、查询代际失效及快照发布。相同凭据、预期 revision 和材料的重复提交返回已完成 revision；不同请求使用旧 revision 时拒绝覆盖。RPC 等待取消不撤销已接收的事务，worker 退出后拒绝新请求。管道和子进程的运行验证尚在补充，不能把编译通过当作贯通证明。

第一次 all-target 接线检查发现 Tokio 句柄接口及任务分发穷尽匹配问题；修正后第二次检查通过，日志 `C:/CodexLocal/wx-workbench-worker-keys-check-2.log`，当时存在两个待删除的未使用函数告警。最终零告警结果尚待重新运行。之后 daemon 定向测试为 359 通过、0 失败、2 忽略，日志 `C:/CodexLocal/wx-workbench-daemon-keys-test-1.log`；包括密钥更新的取消等待、重启持久化和账号配置变化反例，但当时尚未加入私有通道测试，不能据此宣称通道通过。

图片材料写入现已接入同一私有通道：前台 `ImageKeys`、保存模式的 `ImageKeyMonitor` 和授权后的任务 `ImageKey` 使用 daemon 提交材料，不再自行持久化；不保存模式不获取写权限。普通离线图片取钥不再读取完整密钥库。图片监控读取也已接入，见下文；初始化 bootstrap 及其他 worker 的读取仍未完成，不支持 bootstrap，也不会静默切换账号。已迁移路径不再丢弃刚发布的密钥快照；RPC 成功路径刷新任务脱敏材料。其他操作的旧配置刷新尚需随剩余写入路径迁移一并收敛。

图片私有通道定向测试 19 通过、0 失败、1 子进程夹具入口忽略，日志 `C:/CodexLocal/wx-workbench-image-broker-test-1.log`。真实离线 CLI 夹具最初误用不存在的命令参数，修正为正式入口后，发现一次性受监督进程工具在 CLI 结束时也回收了 daemon。现沿用既有入口夹具方式，独立限制 CLI 时间和输出，由账号夹具按认证进程身份清理 daemon；没有放宽 daemon 存活断言或改变生产启动逻辑。移除该夹具对通用进程测试模块的重复引入，原有进程安全测试保留。

最终该离线目标 8 通过、0 失败、0 忽略，日志 `C:/CodexLocal/wx-workbench-image-runtime-test-3.log`。使用本 checkout 的实际 `wx.exe`、合成缩略图和人工 DPAPI 材料，覆盖首次保存、保留其他材料和配置、旧格式/损坏库明确拒绝、只读不保存、样本解码失败不提交以及发布结果；未扫描真实进程或访问真实账户。这不代表监控、初始化或全部根测试已经完成验收。

图片阶段收尾：严格 Clippy 首次发现新夹具重复加载 DPAPI 模块，已改为复用测试支持模块而非增加豁免。随后 `cargo clippy --offline --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings` 通过，日志 `C:/CodexLocal/wx-workbench-image-clippy-4.log`。最终格式检查、all-target `cargo check` 和上述 8 项离线测试均退出 0，分别见 `C:/CodexLocal/wx-workbench-image-format-4.log`、`C:/CodexLocal/wx-workbench-image-check-4.log`、`C:/CodexLocal/wx-workbench-image-runtime-test-4.log`。本阶段未重新运行额外复杂度规则、完整根测试和独立 fixture 矩阵，不能据此宣称全目标完成。

### 图片监控读取收敛

原 `daemon/operations/image_keys.rs` 在监控复用材料时独立 `Store::load`，并二次解密复核版本，与 daemon 快照存在两个读取权威。现已删除该生产路径：daemon 注册监控 worker 时从 `QueryState` 取图片材料，通过现有受限 stdin 帧传递；普通图片取钥及数据库 worker 不获得图片读取材料。`READ_IMAGE` 与图片写权限分离，no-save 监控仅能读，仍必须具有原有显式授权。

私帧中的 `ImageMaterial` 限定 AES/XOR，Debug 脱敏、析构清零；不携带账号或逐库密钥。新增内部 `WorkerKeyRevision` 请求，仅按已认证 pipe PID、原子进程句柄、注册有效性和读权限复核版本，回复只有 `verified`，不返回秘密。等待快照后再次检查撤销和存活。客户端固定父进程代次，并在接收回复后检查 worker 作用域及版本，保留脱敏的冲突/未授权错误。确认写入成功后才更新私有缓存，结果未知时不提前发布新版本。

监控继续实际验证本地模板，保留配置、取消与样本发布检查；只有 `NoImageKeyFound` 可继续等待。缺失材料不合成零密钥，未验证材料不跳过模板验证。监控固定启动材料的版本，并发提交后拒绝旧版本；外部替换密钥文件只有显式失效或重启后加载，不恢复每轮独立解密。没有增加公开工具、服务或秘密环境变量。

验证使用人工材料、临时 DPAPI 库、真实管道和本 checkout 的 `wx.exe`：`cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx worker_keys -- --test-threads=1` 为 26 通过、0 失败、1 原有子进程夹具入口忽略，日志 `C:/CodexLocal/wx-workbench-image-read-broker-1.log`。覆盖只读写入拒绝、错误进程/凭据、撤销、并发更新、显式失效和私帧脱敏。`--test image_keys_runtime -- --test-threads=1` 为 9 通过、0 失败、0 忽略，日志 `C:/CodexLocal/wx-workbench-image-read-runtime-2.log`；新增真实监控复用合成材料，并在缓存建立后损坏临时密文再次成功，证明 worker 不独立解密磁盘。测试不读取真实微信内存，不接触真实账号。

本阶段格式检查、all-target 编译及严格 all-target Clippy 均退出 0，日志分别为 `C:/CodexLocal/wx-workbench-image-read-format-1.log`、`C:/CodexLocal/wx-workbench-image-read-check-2.log`、`C:/CodexLocal/wx-workbench-image-read-clippy-1.log`。入口架构 6 项、实际服务 4 项全部通过（含取消及子进程回收），日志 `C:/CodexLocal/wx-workbench-image-read-entry-1.log`。没有用定向测试替代尚待最终执行的全部根测试和独立 fixture 矩阵。

额外执行 `cargo clippy --offline --locked --target x86_64-pc-windows-msvc --bin wx -- -W clippy::cognitive_complexity`，退出 0 但仍报告四处超阈值：请求分发 50、消息监控 30、延迟监测 31、朋友圈发布 26（阈值 25），与本阶段前相同，本次图片读取链路没有新增超阈值项。日志 `C:/CodexLocal/wx-workbench-image-read-complexity-1.log`。这些额外规则告警不计作严格默认 Clippy 告警，不宣称指标全部归零；后续应随相应责任迁移处理，不机械拆分。

## 追加范围：取消 Toolkit 聚合

`src/toolkit`、根模块声明、生产引用和兼容聚合入口现已移除。其职责按语义迁入 `application`、`adapters/wechat`、`infrastructure`、`web` 和 service 契约；正式 `wx toolkit` 命令分组继续保留，但不再对应源码架构层。迁移没有删除仍有业务价值的功能，也没有建立替代性的通用工具目录。

顺序为完成密钥/worker 一致性边界，抽清公共请求契约与文件基础设施，再将导出、转录、监控、维护等用例逐域接入应用层；微信私有格式进入既有适配器，Web 归协议入口。迁移完成的域必须同时更新第一方调用方、独立夹具、当前文档及架构图源。最终需要逐项功能去向与回归证据，目录消失本身不构成验收。

审查线索来自仓库外的内部只读架构报告，其中取样并非原子代码快照，具体关系须以当前实现复核。历史报告和第三方来源保持原貌。

### 私有文件权限基础设施

发现通信令牌、任务历史、密钥与媒体暂存的权限保护都引用 `toolkit::private_file`，使基础设施反向依赖聚合工具模块。已将实现移至 `src/private_file.rs`，删除旧文件及 Toolkit 模块声明，所有生产调用改为直接引用，不保留转发层。按已打开句柄设置当前用户 ACL 的算法、首次写入前限制权限的顺序及既有权限断言均保留；没有引入文件管理框架或改变磁盘格式。

补充 AST 依赖检查，拒绝私有文件模块引用 Toolkit、daemon、CLI 或业务层；该检查仅验证依赖方向，实际保护行为仍需 ACL 和发布测试。当前架构及 Toolkit 迁移表已同步。此次为内部模块路径变更，正式命令和配置不变，用户无需重新初始化；未触碰任何真实用户文件。

完整主程序单元测试 `cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx -- --test-threads=1`：1402 通过、0 失败、11 原有忽略，日志 `C:/CodexLocal/wx-workbench-private-file-bin-test-1.log`。忽略项为 6 个父测试使用的子进程夹具入口，以及 Frida、3 个 FFmpeg、Windows 符号链接权限共 5 个条件测试；没有新增忽略。模块正文与移动前相比无算法差异，仅模块说明改变。

首次根 all-target 编译发现历史查询运行时的间接支持模块漏接根级声明（`history_runtime`），日志 `C:/CodexLocal/wx-workbench-private-file-check-2.log`，退出 101。补齐该根目标及其独立 runtime 入口后，`cargo check --offline --locked --target x86_64-pc-windows-msvc --all-targets` 退出 0，日志 `C:/CodexLocal/wx-workbench-private-file-check-3.log`。没有恢复 Toolkit 转发层或删除测试来绕过错误。

独立矩阵首轮 27 项中 3 项失败，汇总留存 `C:/CodexLocal/wx-workbench-private-file-fixtures-1-summary.json`：历史/计划夹具此前新增 `zeroize` 后未同步锁文件，图片安全夹具跨 crate 使用配置模块不能访问当前私有配置校验。锁同步后另暴露计划 runtime 的根声明遗漏，已补齐。图片夹具改为直接编译生产 `config.rs/runtime.rs`，按已有版本补充 `dirs = "5"`，不扩大生产校验函数可见性；三个受跟踪锁文件合计仅新增 15 行。手工核验历史夹具一度遗漏矩阵原有的 `CARGO_BIN_EXE_wx` 环境，补回本 checkout 路径后恢复 `--locked` 验证。

最终 `scripts/check-fixtures.ps1 -Offline` 27 项编译检查全部退出 0，日志 `C:/CodexLocal/wx-workbench-private-file-fixtures-3.log`。这不是 27 项运行测试。另实际执行独立 `wav-publish --test publisher`（1 通过）和 `mcp-image-security --test audit`（20 通过、2 个原有符号链接权限忽略），日志 `C:/CodexLocal/wx-workbench-private-file-wav-test.log`、`C:/CodexLocal/wx-workbench-private-file-image-audit.log`。未使用真实账号或外部转录服务。

格式首轮仅发现已改测试模块声明排序差异，按文件格式化后 `cargo fmt --all -- --check` 通过，日志 `C:/CodexLocal/wx-workbench-private-file-format-2.log`；严格 all-target Clippy 通过，日志 `C:/CodexLocal/wx-workbench-private-file-clippy.log`。本阶段未重新测量额外复杂度规则，权限算法未改，上一阶段四处复杂度热点仍列为待处理；未把默认零告警作为额外规则归零的证明。完整根集成测试仍待最终集中验收。

最后运行根 `entry_architecture`（7 通过）与 `history_runtime`（1 通过），日志 `C:/CodexLocal/wx-workbench-private-file-entry-tests.log`，验证新增依赖约束和漏接修正后的实际合成账号历史查询。初始化/bootstrap 的目标冻结和密钥、配置两步发布仍未迁移，不以本次文件基础设施迁移替代该目标。

## 初始化配置发布恢复

已修复：`daemon/operations/init.rs` 的非 force 复用分支原先看到数据库密钥便直接报告已初始化，可能遗漏上次失败后尚未发布的 `key_store` 等配置字段。现在在判断复用前取得配置锁，复用材料与新扫描成功统一调用受保护的配置提交步骤；只有完成所需提交才报告成功。配置仍保留未知字段并复核原始字节快照；密钥与配置仅各自原子发布，不是跨文件事务。扫描算法及授权不变。

新增真实 CLI 夹具 `tests/init_runtime.rs`：使用临时配置、人工 DPAPI 材料及虚构进程名，锁住配置文件制造发布失败，确认明确失败、原密文和配置不变；解除锁后重试补齐声明，保留无关字段，不执行扫描、不产生明文镜像。第一次运行的业务断言完成后，夹具因一次性 Job 回收 daemon 与账号清理竞争而失败（`C:/CodexLocal/wx-workbench-init-recovery-test.log`），没有将其记为通过。

随后抽取图片入口测试已经使用的有界 CLI 捕获为 `tests/support/cli_output.rs`，由调用方 `RuntimeCleanup` 负责 daemon 清理，助手仅拥有 CLI 子进程，并在超时、输出超限或异常时回收该子进程。初始化与图片两处复用，无生产生命周期改变。第二轮两个目标 10 项全部通过，日志 `C:/CodexLocal/wx-workbench-init-recovery-test-2.log`；之后加强失败原因断言，必须含配置提交失败，避免无关启动错误造成假阳性。此修复不替代初始化/其他 worker 的 daemon 密钥所有权迁移，该项仍未完成。

加强后的初始化回归再次通过，日志 `C:/CodexLocal/wx-workbench-init-recovery-test-3.log`；格式、all-target 编译与严格 all-target Clippy 均退出 0，分别见 `C:/CodexLocal/wx-workbench-init-recovery-format.log`、`C:/CodexLocal/wx-workbench-init-recovery-check-2.log`、`C:/CodexLocal/wx-workbench-init-recovery-clippy.log`。本阶段未重跑完整根测试、独立矩阵或额外复杂度规则，不复用上一阶段结果声称本次最终全量通过。

## 初始化已绑定账号窄切片

在现有私有 worker key broker 上接入 `Initialize(provider=Memory, force=false)`：注册时要求当前完整配置与 `ConfigDocument::with_db` 结果一致、无目录覆盖、非 bootstrap；仅下发 `InitSeed { has_database_keys }`，不下发账号主密钥或数据库材料。扫描结果以现有 `MaterialChange::Databases` 经过进程绑定、revision CAS、幂等和原子 `QueryState::update_keys` 提交，初始化 worker 保持 access 至配置发布结束。`force`、`Auto`、`Account`、目录覆盖和 bootstrap 明确拒绝或不给 grant。

新增 `worker_keys` 授权边界测试覆盖上述组合，最终 27 通过、0 失败、1 个原有子进程入口忽略，日志 `C:/CodexLocal/wx-workbench-init-broker-test-6.log`。测试使用合成配置、临时数据库目录和挂起 worker，不运行 scanner。此前测试误把目录覆盖的配置错误当作“无 grant”，已改为断言明确失败，避免削弱账号隔离。

本段是当时的窄切片状态。后续已删除 `PreparedDecrypt`/`PreparedEmoticons` 公共变体，正式操作直接消费 daemon 只读快照；首次初始化、其他 worker 的材料读取和剩余 `Store::load/update` 路径仍应以当前源码及最新验收报告为准。
