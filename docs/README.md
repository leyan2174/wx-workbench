# 文档索引

## 使用

- [项目与命令入口](../README.md)：安装、账号选择、查询、导出、MCP 和 Web。
- [正式命名与安装方式](project-naming.md)：wx-workbench 名称、已移除的旧发布入口及外部发布步骤。
- [账号密钥](account-key-provider.md)：provider、DPAPI、权限与显式重启。
- [工作流条件](legacy-workflow-gap-audit.md)：媒体、转录、下载、更新和清理的前置条件。
- [附件契约](native-attachment-contract.md)：消息定位、资源匹配与发布。
- [MCP 协议](../src/mcp/PROTOCOL.md)：工具参数、限额、会话与错误。

## 实现

Toolkit 职责拆分已经完成，旧 `src/toolkit` 架构层已删除；正式 `wx toolkit` 命令分组继续保留。查询和 worker 的运行材料由 daemon 快照统一提供，初始化 bootstrap 是仍被明确保留的存储创建边界。

- [架构](architecture.md)、[入口边界](daemon-entrypoints.md)、[后台任务](daemon-tasks.md)。
- [架构图](diagrams/README.md)：图源与生成约束。
- [音频基础设施](../src/infrastructure/audio/README.md)与[批量音频](voice-batch-export.md)。
- [音频边界优化报告](architecture-optimization-audio-2026-09-16.md)。
- [ASR 后端](asr-backends.md)、[本地 ASR](../src/infrastructure/transcription/LOCAL.md)、[云端授权](../src/infrastructure/transcription/OPENAI.md)、[缓存](../src/application/transcription/CACHE.md)、[数据库音频](../src/adapters/wechat/media/VOICE_DATABASE.md)、[回写](../src/application/transcription/WRITEBACK.md)。
- [SNS 媒体密钥流](../src/adapters/wechat/media/SNS_KEYSTREAM.md)。

## 维护

- [当前正式契约清理](current-contract-cleanup-2026-09-16.md)：本轮实际改动、破坏性变化、当前实现与待完成范围。
- [架构清理与最终验证](architecture-cleanup-final-2026-09-16.md)：最终职责、移除的兼容机制、复杂度、全量测试与保留限制。
- [架构优化实施与验收](architecture-optimization-2026-09-16.md)：解析资源归属、密钥材料生命周期、监控恢复回归及本阶段检查证据。
- [SNS 媒体适配器迁移](sns-media-adapter-2026-09-16.md)：密钥流实现归属、直接调用接线及本轮独立验证。
- [MCP 夹具配置复用](fixture-config-reuse-2026-09-16.md)：消除重复配置模块与依赖告警，保留账号固定验证。
- [远端表情格式迁移](emoticon-format-adapter-2026-09-16.md)：CBC、格式识别与流定位归属，保留下载发布和错误阶段。
- [分阶段质量检查](quality-checks.md)：中途全目标检查、全量测试和独立夹具的复现方式及日志边界。

- [2026-09-16 代码质量修订](code-quality-review-2026-09-16.md)：改名、复杂度、告警、抽象边界与验证范围。
- [测试说明](../tests/README.md)。
- [测试与整理计划](testing-plan.md)。
- [开发与回归验证](rust-migration.md)。
- [第三方来源与许可](../THIRD_PARTY_NOTICES.md)。
- [DMCA 与文档发布风险](dmca-and-publication-risk.md)：来源仓库状态、反规避争议及技术文档的发布边界。

文档只说明当前接口与限制。私人账号样本、本机路径及每轮测试日志留在仓库外。
