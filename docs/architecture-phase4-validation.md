# 架构第四阶段验证记录

本阶段基于已推送的 `5ef856e787bb2f9be0120a43438c011b86892117`，继续实现整仓业务与微信适配边界。以下是阶段性证据，不是整仓目标完成声明。

## 生产接线

- 原始联系人元数据迁入 `adapters/wechat/contacts/raw_export.rs`，标签值解析由同目录共享模块拥有。全量与增量原始导出直接调用适配器，删除 toolkit 的替代实现。原始数值和警告回退保持兼容，普通 Contact/Tag 不接收动态 JSON。
- `business/chat_plan.rs` 拥有消息统计、有序来源贡献、累计及语义化部分结果。`adapters/wechat/planning` 拥有 SQL、缓存清单和具体媒体目录规则。toolkit 保留线程调度及 CSV 投影，旧状态字符串映射和排序留在展示边界。
- 批量音频使用显式 legacy 语音来源策略及联系人批量投影。业务层仅拥有选择、结果、唯一计数结构 `BatchProgress` 和状态；原始物理标识及旧失败 JSON 留在适配器和宿主。报告通过 `serde(flatten)` 复用计数，不另维护一份成功状态。
- 批量 MP3、单文件 MP3、目录归属信息与 WAV 最终产物复用 `ExportTarget`。WAV 保留宿主回调前后的字节校验、响应预算、账号及取消检查；没有迁移同步 MCP 语音的生命周期。

## 兼容边界

- 容量统计保留 SQLite 原有长度语义、包含两端的时间区间、媒体首错停止及已有贡献。扫描仍统计声明缓存范围，不包装成完整账号库存或严格媒体关联。
- 批量语音保留四列旧 schema、排序及重复记录、`unknown_ID`、先跳过已有 MP3 再读取材料，以及逐条失败。原始诊断标识不是跨库或跨账号的业务身份。
- 原始联系人元数据允许的数值投影不强加给严格标签查询；全量与增量既有的类型转换差异保留，并由测试锁定。
- 任务仍使用现有 worker Job 回收。没有新增整批期限或 SILK 解码中途取消；强杀不能保证所有 Rust 临时文件析构运行。已发布文件也不因后续失败自动回滚。

## 验证过程

所有测试使用临时配置、合成数据库、人工材料及本机受控进程；未访问真实账号、扫描微信内存、上传音频或下载私人媒体。构建使用已有 libclang 和锁定的 Windows x64 MSVC 工具链。

日志保存在本机 `C:/CodexLocal/`，不提交构建日志或测试输出文件。

| 检查 | 结果 | 日志 |
| --- | --- | --- |
| WAV 专用真实发布套件 | 通过；覆盖回调拒绝、临时内容篡改、目标竞争和目录替换 | `wx-cli-phase4-wav-publish-1.log` |
| 第一轮编译检查 | 通过 | `wx-cli-phase4-check-1.log` |
| 第一轮测试构建 | 失败，测试未运行：迁移后测试入口、Pin 夹具接线和 SQLite 辅助方法需修正 | `wx-cli-phase4-test-1.log` |
| 第二轮编译检查 | 通过，33 条警告 | `wx-cli-phase4-check-2.log` |
| 第二轮全量测试 | 通过，25 个目标、2648 条通过记录、23 条忽略、零失败；不同目标有重复覆盖，不是唯一用例数 | `wx-cli-phase4-test-2.log` |
| 原始联系人独立夹具 | 通过，26 条通过记录 | `wx-cli-phase4-fixture-contact-rows.log` |
| MCP 只读安全独立夹具 | 通过，15 条通过记录 | `wx-cli-phase4-fixture-mcp-readonly-security.log` |
| ASR 缓存安全独立夹具 | 通过，329 条通过记录、4 条忽略 | `wx-cli-phase4-fixture-asr-cache-security.log` |
| MCP 语音断连独立夹具 | 通过，1 条通过记录 | `wx-cli-phase4-fixture-mcp-voice-host.log` |
| MCP 语音宿主安全独立夹具 | 通过，19 条通过记录、1 条忽略 | `wx-cli-phase4-fixture-mcp-voice-host-security.log` |

未删除或放宽旧行为、安全断言来解决编译问题。新增测试同时覆盖 raw 与严格类型差异、CSV 状态映射、批量报告完整 JSON 等值和共享发布行为。

## 后续具体项

只读复核确认以下仍有真实生产调用，下一阶段继续处理：

1. `daemon/meta.rs` 的普通消息库存目录识别。
2. `daemon/cache.rs::latency_probe` 的 Session schema 读取。
3. `daemon/query/export_directory/catalog.rs` 中未映射原始目录身份的重复物理校验。
4. `application/attachment_references.rs` 的受控附件引用选择、`adapters/wechat/media/attachment_content.rs` 的私有 XML/缓存布局，以及 `attachment/resolver.rs` 的旧 DAT 月份与优先级规则。
5. `toolkit/chat_directory/media.rs` 的媒体 XML 语义。
6. ASR `prepared_audio.rs` 和 `receipt.rs` 的物理来源证明校验。
7. `daemon/query/mcp_refer.rs` 的严格引用回复解析。

通用受限 XML 加载、压缩、哈希、文件身份检查、原始格式投影及宿主装配不是这份剩余清单的同义词；不为目录整齐而重写现有算法。
