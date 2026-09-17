# 架构重构集中验收

> 阶段记录：本文保留实施当时的路径、限制和测试结果，不是当前接口规范。当前职责与入口以[架构说明](architecture.md)和[文档索引](README.md)为准。

本记录对应 `5115419` 之后的收尾改动。它更新开发阶段文档中的“待集中验证”状态，不抹去此前失败记录。所有验证使用隔离配置、合成 SQLite/XML、人工密钥、本地测试 HTTP 服务和受监督的测试进程；未访问真实微信账号、扫描真实微信进程、重启微信或上传私人内容。

## 当前依赖与接线

依赖方向为：CLI/HTTP/MCP 与执行宿主消费业务契约；微信适配器实现业务窄接口；daemon 使用固定 `RuntimeContext`、`QueryLease` 和 `DbCache` 装配适配器。业务模块不反向依赖 SQLite、clap、daemon 或微信适配器，也不拥有动态 JSON 协议响应。公开请求由 service/IPC 契约拥有，CLI 保留解析、授权和展示。

| 业务域 | 权威实现及接线 | 兼容和完整性边界 |
| --- | --- | --- |
| 联系人、成员、标签 | `business/contacts.rs`、`adapters/wechat/contacts`、daemon 联系人查询；CLI/MCP/HTTP 继续经服务入口 | 同名不是身份；主来源错误不触发另一拼写的静默回退；观察到的群成员不宣称完整成员表。 |
| 会话、历史、搜索、新消息 | `business/sessions.rs`、`business/messages`、微信消息适配器、`daemon/query/message_read.rs` | 消息库存不依赖会话摘要；端点、物理排序和候选预算在适配器；满页仅表示可能有下一页。旧 wire 由显式兼容投影保留。 |
| 结构化消息、引用、通话事件 | 业务结构化内容及消息契约、微信消息解析/`reply_read.rs`，daemon 只保留快照和协议投影 | 私有压缩/XML 在适配器；引用旧七字段不变；不能由通话文字推测音视频类型或录音存在。 |
| 图片、语音、附件 | 业务媒体/语音契约、微信媒体适配器、严格消息证据、现有 codec/ASR/发布宿主 | 普通语音页使用业务类型和不透明来源；旧物理字段只在兼容投影。严格关联不接受历史启发式回退。语音消息不是通话录音。 |
| 朋友圈 | `business/moments.rs`、微信 timeline/cache 适配器、既有 SNS 编排 | RecordedCompatibility 与 Effective 作者策略显式区分；缓存恢复输出由类型化 writer 发布；缺库返回 Unavailable，空库与缺库不同。只代表本地缓存。 |
| 收藏 | 业务收藏类型、微信收藏适配器、daemon 查询和 CLI 投影 | 旧数字归兼容契约；旧 stdout 数组保持；缺结果字段拒绝；分页未结束在 stderr 提示，不发明 offset 能力。 |
| 公众号文章 | 业务文章操作、完整官方推送快照和微信文章适配器 | 未映射分片报告部分结果；损坏数据不是空结果；URL 不作为稳定身份。 |
| 表情 | 业务选择/批处理结果、微信目录适配器和现有下载发布器 | 成功文件保留、继续处理其他项；部分失败退出 20，全失败退出 1。纠正旧“全部失败仍成功”行为；缓存名称不构成密码学关联证明。 |
| 全量/增量导出、归档、计划 | 业务 archive/chat plan、微信来源适配器、既有流式转换、共享 publisher/index | 不将 worker 流聚合为万能响应；产物发布后才推进索引；索引失败不推进内存状态；多文件不是跨文件事务。 |

这次删除或替换的生产逻辑包括 daemon 中的数据库库存分类、原始消息页构造、原始语音目录返回、严格引用记录物化，以及宿主中的 SNS 缓存格式和图片批次布局规则。原算法移入实际适配器并切换调用方，不保留第二套查询/任务执行器。

`toolkit` 仍拥有文件、音频、HTTP 和执行编排；没有简单改名为 domain。保留的 `mcp_*` 查询门面承担账号快照、宿主边界或旧投影，不代表纯业务规则仍归 MCP。`LegacyVoiceMessage`、`LegacyReadPolicy`、原始导出和来源诊断是明确的兼容边界，不能用其物理字段反向定义普通业务身份。同步 MCP 语音工具没有迁入持久任务队列。

## 密钥、发布与生命周期

- 账户主密钥、逐库密钥和图片材料经同一类型化 DPAPI 存储入口，绑定账号、版本和变更序号。获取器只返回经验证材料；初始化复核宿主后，以观察到的 revision 一次保存主密钥与逐库密钥，避免分开提交。
- 旧材料迁移先验证并成功保存受保护材料，再更新引用；保留无关配置。损坏受保护存储不回退明文，不新建明文镜像或秘密备份。原有显式 unverified 导入只表示未验证兼容材料，不冒充已验证获取结果。
- `keys_file` 和配置绑定沿用共享 runtime/migration 协调；更新排空旧查询租约及缓存工作，再失效代际。运行中的入口不能静默切换账号。
- `application::publication_context::PublicationContext` 向共享发布器提供配置、密钥、源、缓存和运行目录保护；合法离线入口仅在配置缺失时使用明确离线模式，损坏配置不回退。数据库快照有配置缓存输出的窄例外，普通导出没有该例外。
- 单音频的 PCM 暂存、编码及提交均位于发布器目录保护期间。取消、超时和输出上限复用受监督进程；发布前复核宿主。音频已发布而转写失败仍是阶段性结果。
- 查询、前台 Operation、持久任务保持不同生命周期。前台操作失联按轮询租约收尾；查询断连不保证立即中止已开始的缓存工作。持久任务由 daemon 管理，MCP/HTTP 断连不等于任务取消。
- MCP `submit_task/list_tasks/get_task/cancel_task/get_task_events` 复用 `service::client`、共享任务协议、`daemon::tasks` 和原 worker。宿主允许集与 daemon 能力取交集；模型布尔值不能授予扫描、写出、上传或任意路径权限。
- 提交回复丢失或超时时结果可能未知，应使用同账号、原幂等 ID 和同请求重试；不是重新生成 ID。只有显式任务取消才取消后台任务，已发布副作用不回滚。重启后的未完成任务为 `interrupted`，不会自动续跑。幂等和历史保留有容量限制。

## 根工程验证

环境：Windows x64 MSVC，`LIBCLANG_PATH=C:/CodexLocal/build-tools/libclang/clang/native`；没有删除 silk-codec 或跳过相关功能。构建产物在仓库外的 `C:/CodexLocal/wx-quality-target-20260908`。

| 命令范围 | 实际结果 | 完整日志 |
| --- | --- | --- |
| 最终 `cargo check --offline --locked --target x86_64-pc-windows-msvc` | exit 0，仍有现存警告 | `C:/CodexLocal/wx-cli-audit-delivery-check.log` |
| 集中 `cargo test --offline --locked --no-fail-fast --target x86_64-pc-windows-msvc -- --test-threads=1` | 25 个目标，2839 passed、5 failed、23 ignored；exit 101 | `C:/CodexLocal/wx-cli-audit-final-root-tests-2.log` |
| 四个受影响目标的 `process_tests` 定向复测 | 全部通过，exit 0 | `C:/CodexLocal/wx-cli-audit-audio-fix-tests.log` |
| `emoticons_runtime` 复测 | 6 passed，exit 0 | `C:/CodexLocal/wx-cli-audit-emoticons-fix-tests.log` |

四个失败来自同一音频错误包装回归，已恢复编码错误原文而保持目录锁及输出隐藏断言。另一项是表情旧测试仍期待全失败成功退出，已按新业务失败契约要求 exit 1，并保留统计文本和无失败产物断言。首次集中命令仍记录为失败，不能改写为“一次全量通过”。重复编译的生产模块在不同目标中重复计数，以上不是唯一测试数。

根测试实际通过的关键证据包括：`architecture_contracts` 3 项、`business_contracts` 43 项、`entry_architecture` 4 项；真实双账号 Web 三查询路由、MCP 宿主授权拒绝、CLI/Web 同任务、真实 worker 导出与重启历史、代际排空、密钥迁移与损坏拒绝，以及数据库/归档/媒体输出保护测试。对应位置见 [入口证据](architecture-entry-acceptance.md) 和 [完整验收清单](architecture-acceptance.md)。

## 独立夹具验证

首次串行批次 26 个夹具中 23 个 exit 0；以下记录范围及失败后的定向修复，不将编译失败算作零用例通过。默认命令为 `cargo test --offline --manifest-path tests/fixtures/<name>/Cargo.toml --target x86_64-pc-windows-msvc -- --test-threads=1`。完整日志前缀为 `C:/CodexLocal/wx-cli-audit-fixture-`，首次结果保存在同目录 `wx-cli-audit-fixture-results.json`。

| 夹具 | Passed / Ignored | 范围或复测 |
| --- | --- | --- |
| attachment-refs | 87 / 0 | 全部 |
| native-image | 96 / 0 | 全部 |
| mcp-image-listing-parity | 17 / 0 | 全部 |
| mcp-protocol | 52 / 0 | 包含 stdio 子进程 |
| mcp-readonly-security | 15 / 0 | 全部 |
| mcp-image-security | 20 / 2 | 全部，辅助进程不计通过 |
| native-attachment-security | 57 / 0 | 全部 |
| mcp-audio | 183 / 0 | 全部 |
| mcp-voice | 21 / 0 | 全部 |
| mcp-voice-security | 102 / 0 | 补真实本地文件保护模块及依赖后；`-fix-2.log` |
| mcp-voice-host | 1 / 0 | `--test shutdown`；共享生产模块另由根目标验证 |
| mcp-voice-host-security | 19 / 1 | `--test audit` |
| wav-publish | 1 / 0 | `--test publisher` |
| asr-cache-security | 355 / 4 | 全部 |
| emoticons-catalog | 83 / 0 | 全部 |
| emoticons-download | 138 / 2 | 包含受监督转换器 |
| sns-download | 99 / 1 | 全部 |
| mcp-cli | 32 / 0 | 全部 |
| asr-video-security | 355 / 3，另 2 项复测通过 | 首次缺独立被测程序路径；设置 `WX_SECURITY_WX_EXE` 为同次构建的 `debug/wx.exe` 后，仅重跑 `security_cli_`；`-fix.log` |
| asr-local | 24 / 1 | 修正模块根并接入真实进程监督及依赖后；`-fix-2.log` |
| sns-video-native | 9 / 0 | 全部 |
| mcp-history-compat | 4 / 0 | 全部 |
| contact-rows | 26 / 0 | 全部 |
| sns-publish | 32 / 0 | 全部 |
| sns-album-render | 6 / 0 | 全部 |
| plan-selection | 3 / 0 | 全部 |

语音目录独立夹具只替代 DbCache 的公开接口，执行真实业务/SQLite 适配器；它不是实际解密集成的替代证据。真实解密、账号及 daemon 链路由根运行时目标另行覆盖。独立夹具新增的锁依赖没有升级既有锁定包。

## 限制与交付边界

已发现的根测试及独立夹具失败均有对应修复和复测，没有未解决的已知测试失败。未执行真实账号验收、真实内存获取、微信重启、真实云 ASR 或私人媒体下载，这是本次要求的安全边界，不据合成测试保证所有未来微信版本。

忽略项不计通过；现存编译警告未在架构收尾中顺带清理。未将查询、操作和任务合并，也不承诺跨库原子快照、跨文件事务、无限期幂等、同用户恶意代码沙箱或任务自动断点续跑。结构变化应主要在微信适配器处理；能力确实消失或格式未知时，通过明确错误/完整性状态影响上层，不能伪造空成功。
