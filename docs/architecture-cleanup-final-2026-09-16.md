# 架构清理与最终验证记录（2026-09-16）

## 基线与范围

- 当前 checkout：`C:\Users\leyan\Documents\skills\_external\wx-workbench`。
- 分支与基线：`main`，读取时 HEAD 为 `d4e22a0`；工作区已有大量未提交修改，本轮没有重置、覆盖或合并其他修改。
- 远端：`https://github.com/leyan2174/wx-workbench.git`。
- 支持目标：Windows x64 MSVC；构建目录固定为本 checkout 专属的 `C:\CodexLocal\wx-workbench-target-20260916`。
- 检查范围：复杂度热点、Toolkit 职责迁移、业务/微信适配边界、正式命令与执行契约、数据库解密验证边界、入口夹具及当前文档。
- 所有运行验证均使用合成 SQLite、人工材料、回环 HTTP 和临时目录。未访问真实微信账号、聊天数据或密钥，未扫描微信进程，未调用云端转录或上传私人媒体。

## 已修复

### Toolkit 架构层

`src/toolkit`、根模块声明及生产兼容转发已经删除，没有整体改名为另一个通用工具目录。职责按真实所有者拆分：

| 责任 | 当前所有者 |
| --- | --- |
| 业务对象、规则、完整性与部分结果语义 | `src/business` |
| 导出、监控、转录、清理和媒体工作流 | `src/application` |
| 微信表、分片、XML/BLOB、压缩和媒体格式 | `src/adapters/wechat` |
| 文件发布、配置事务、音频、转录后端、进程监督 | `src/infrastructure` |
| HTTP/SSE 与静态资源 | `src/web` |
| 账号租约、密钥快照、查询、任务和 worker 生命周期 | `src/daemon` |

`src/cli/toolkit.rs` 与 `src/daemon/operations/toolkit.rs` 继续存在，因为 `wx toolkit` 是当前正式命令分组；它们不是被删除的 Toolkit 架构层。生产搜索没有发现 `crate::toolkit` 或 `src/toolkit` 依赖，架构测试保留旧路径字符串作为禁止回归的反例。

### 业务与微信适配边界

- 联系人、会话、消息、附件、朋友圈、收藏、文章、表情和语音目录使用按域划分的业务对象及结果语义；微信 schema、分片和私有格式留在 `adapters/wechat`。
- 朋友圈 SQLite/XML 快照和作者证据由微信适配器生成，应用层负责筛选、下载授权和发布；发布前完成批次预检，失败不将不完整结果声明为完整成功。
- 消息目录只消费已经解析的消息和不透明媒体证据，不直接读取微信表；物理 `local_type` 仍只在原始/兼容投影和少数媒体增量路径出现，普通业务身份不依赖它。
- 聊天目录的图片材料由 daemon worker broker 按固定账号和 revision 只读交付，应用接收窄 `MediaInput`，不再打开或解释密钥库。前台 Operation 与持久任务复用同一个导出实现；dry-run/no-media 不获得材料权限。
- SNS 缓存归档也改为从同一 broker 取得只读图片材料，`export_for` 只接收 `CacheKeys`；前台归档与 `SnsArchive` 任务共用实现，均不具备数据库读取或密钥写入能力。
- 语音数据库关联、远端表情格式和 SNS WxIsaac64 密钥流各有一个微信适配器权威实现；转录应用不反向拥有 WAV/SILK、进程或 HTTP 后端。
- 数据库导出应用只协调已提供的密钥、来源固定和发布。SQLite `PRAGMA quick_check` 位于基础设施验证器，`application::database_decryption` 不依赖 `rusqlite`。

### 执行与正式契约

- CLI、Web 与 MCP 继续经 service/daemon 使用查询、前台 Operation 或持久任务；没有在入口新增队列、worker 或业务副本。
- `toolkit_run_prepare.rs` 已改名为 `database_key_validation.rs`，名称反映其真实职责。
- 删除公共 `PreparedDecrypt`、`PreparedEmoticons` 过渡操作，以及 `ExportAll.prepare`、`ExportAll.announce` 无效组合。
- 解密只保留严格语义：缺少数据库密钥或逐项失败均明确失败，不再使用旧的宽松成功模式。
- daemon 的原生命令清单只列当前正式命令，测试断言任何条目都不再以 `run ` 开头。

## 移除的兼容机制

| 已删除 | 当前唯一方式 | 调用方与用户影响 |
| --- | --- | --- |
| `wx toolkit run ...` | 直接使用 `wx toolkit <command>` | 第一方测试、帮助和文档已更新；旧调用明确报参数错误。 |
| `wx toolkit export-chats` 包装命令 | `wx toolkit export-all` 或对应正式导出命令 | 不再保留二次解析 CLI 的外壳。 |
| `PreparedDecrypt` / `PreparedEmoticons` | daemon 快照与正式类型化 Operation | service、daemon、worker 和测试已同步。 |
| `ExportAll.prepare` / `announce` | `ExportAll` 的单一正式请求 | 不再允许无效参数组合。 |
| 解密 `Legacy` 缺钥成功策略 | `wx toolkit decrypt` 严格失败 | 旧缺钥任务需先按当前机制初始化。 |
| ASR `Local` 等兼容别名 | `whisper_cpp`、`python_whisper` 等当前规范名 | 配置与请求测试验证旧别名被拒绝。 |
| 旧密钥迁移命令、明文/旧格式回退 | 当前 DPAPI 账号密钥库与显式初始化 | 不删除用户磁盘旧文件；旧格式不再读取，损坏存储不回退，用户需重新初始化。 |
| 密钥 provider `auto` 及缺材料后扫描降级 | 默认 `saved`；扫描必须显式 `memory`，重启捕获必须显式 `account` | `saved` 只读取 broker 交付的已验证账号材料；首次初始化需明确选择获取来源，旧 CLI/wire 值拒绝。 |
| MCP `start_time` / `end_time`、`search_messages.chat_name` | `since` / `until` Unix 秒与 `chats` 字符串数组 | MCP schema、协议测试和文档已同步；旧字段按未知属性拒绝。 |
| MCP 类型词 `emoji` / `voip` / `app` / `namecard` | `sticker` / `call` / `link` / `file` 等正式业务词，或精确数字类型 | CLI 独立契约不变；MCP 不再猜测宽泛旧词。 |
| HTTP 查询 `username` 字段别名 | `chat` | Web 解析测试验证旧字段拒绝。 |
| chat-plan 清单 `display_name` / `kind` 字段别名 | `chat_name` / `chat_type` | 清单启用未知字段拒绝，第一方 fixture 已更新。 |
| `vendor/wechat-decrypt` 与仓库内旧 Python 业务脚本 | Rust 业务、微信适配器及明确选择的 ASR 后端 | 第三方来源记录仍保留；当前运行不依赖该目录。 |
| `src/toolkit` 兼容 facade | `business` / `application` / `adapters` / `infrastructure` / `web` | 所有第一方生产调用已切换，目录已物理删除。 |

这些破坏性变化不授权删除用户磁盘上的旧配置、密钥、数据库或导出物；程序只是不再将它们作为当前输入格式静默采用。

## 复杂度

额外规则使用 `-W clippy::cognitive_complexity`、阈值 25。历史接手值来自较早报告，不是对旧提交重新构建；当前值来自 `C:\CodexLocal\wx-workbench-complexity-20260916.log`。

| 热点 | 历史接手值 | 当前值 | 处理 |
| --- | ---: | ---: | --- |
| `daemon::server::dispatch` | 50 | 49 | 保留集中协议路由，避免间接注册表和跨模块回调。 |
| `application::monitor::run_monitor` | 30 | 26 | 输出后提交与游标推进责任已收敛；不完整批次不推进。 |
| `application::monitor::latency::run_latency` | 31 | 27 | Sessions 回包校验和候选状态合并提取为纯函数；整批成功后才提交状态。 |
| 朋友圈发布 | 26 | 未超过 25 | 批次验证、暂存与发布责任迁移后退出告警集合。 |

当前额外规则报告 3 个生产告警，命令退出 0。它们不属于默认 Clippy 告警，也没有通过 `allow`/`expect` 隐藏。继续仅为分数拆分会降低协议路由或监控状态机的可追踪性，因此本轮明确保留。

## 验证结果

- `cargo fmt --all -- --check`：通过。
- `cargo check --locked --target x86_64-pc-windows-msvc --all-targets`：通过，无默认编译告警。
- `cargo clippy --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings`：通过。
- `cargo test --locked --target x86_64-pc-windows-msvc`：当前最终源码完整重跑 33 个测试目标，合计 2923 通过、0 失败、20 忽略。
- `pwsh -NoProfile -File scripts/check-fixtures.ps1`：27 个独立 fixture 编译检查全部退出 0；这是编译矩阵，不表述为 27 组运行测试。Windows PowerShell 5 会把 Cargo 的普通 stderr 进度误判为终止错误，因此正式矩阵使用脚本所需的 PowerShell 7。
- 旧入口拒绝、严格解密、架构依赖、MCP/CLI/Web 共享任务、账号隔离、断连重试、重启后 `interrupted`、取消后进程树回收和原子发布均有合成运行测试覆盖。
- 密钥测试还覆盖 `saved` 不交付未验证账号材料，以及授权的 `account` 初始化通过进程绑定 broker 在同一 revision 原子提交账号材料与逐库材料；提交失败不会产生半份存储。

忽略项包括要求宿主 FFmpeg/ffprobe、Windows symlink 权限或显式 Frida 原生进程的集成测试，以及仅由父测试启动的子进程夹具。没有删除失败测试、扩大 ignore 范围或放宽业务断言。

完整根测试日志：`C:\CodexLocal\wx-workbench-test-20260916.log`；all-target check、严格 Clippy 和额外复杂度日志分别为 `C:\CodexLocal\wx-workbench-check-20260916.log`、`C:\CodexLocal\wx-workbench-clippy-20260916.log`、`C:\CodexLocal\wx-workbench-complexity-20260916.log`。fixture 摘要：`C:\CodexLocal\wx-workbench-fixtures-target-20260916\quality-fixtures\summary.json`。

## 保留问题与未验证范围

### 未验证

- 未使用真实微信版本、在线 WAL、真实账号密钥或大型私人数据库验证 schema 覆盖和导出性能。
- 未执行需要本机 FFmpeg/ffprobe、symlink 权限或显式 Frida 进程的忽略测试。
- 未运行实际本地模型质量评估或任何云端上传。
- 多文件输出树按文件提交，不提供整个目录的跨文件原子事务；文档和结果语义保持这一限制。

### 仍未完成

- 运行期密钥材料已有单一正式存储和 daemon 快照；聊天目录、SNS 时间线与缓存归档、前台及任务图片发布均已接入 broker。初始化 bootstrap 仍由 daemon 直接创建正式存储，外部文件替换也仍要求显式失效或重启后加载；不能把这一边界表述为自动热重载。
- 聊天目录和原始导出中仍有少量物理类型/来源字段用于诊断、媒体证据或既有 wire 投影；普通业务对象已隔离，但这些边界仍需逐项审计，不能粗暴删除。
- `dispatch`、`run_monitor` 和 `run_latency` 仍高于额外认知复杂度阈值；延迟监控已按状态提交边界降至 27，其余不为指标机械拆分。

### 仅提出建议

- 若未来需要把初始化拆成独立服务，再评估从 daemon bootstrap 中抽离存储创建；当前没有第二个持久化所有者，不为目录统一增加接口。
- 若未来新增第二种查询协议，再考虑从 `dispatch` 提取共享路由描述；当前单一 wire 协议没有足够复用需求支持注册框架。

## 结论

Toolkit 源码架构层已完成删除，业务、微信适配、应用编排、入口和基础设施的依赖方向比基线清晰；正式命令及 daemon 生命周期仍保留。运行期密钥读取与更新已由 daemon 快照和进程绑定 broker 统一，初始化 bootstrap 仍是明确的 daemon 存储创建边界。当前验证证明本轮改动在合成 Windows MSVC 环境中没有发现回归；真实微信数据覆盖仍是明确未验证项。
