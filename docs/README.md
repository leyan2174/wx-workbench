# 文档索引

本索引区分当前用法、架构约束与历史证据。以实际源码和当前命令 `--help` 为准，不从旧日志数字、文件名中的 current 或历史 TODO 推断当前能力。

## 当前使用

| 文档 | 适用内容 |
| --- | --- |
| [项目 README](../README.md) | Windows MSVC 构建、账号初始化、查询、导出、工具箱、Web/GUI 与运行依赖 |
| [Agent 使用说明](../SKILL.md) | 自动化调用、账号隔离、输出与上传边界 |
| [架构](architecture.md) | 入口、领域模块、账号与权限边界、详细流程及时序图；历史段落均单独标识 |
| [Daemon 任务服务](daemon-tasks.md) | CLI/Web 共享任务、固定配置、幂等、取消、停机、日志与恢复边界 |
| [架构图目录](diagrams/README.md) | 当前静态图、图源、生成方式及历史图适用范围 |
| [迁移验收记录](rust-migration.md) | 九项目标、最新自动化回归、精简清理、真实环境未验证边界；旧阶段计数只作历史证据 |
| [测试入口](../tests/README.md) | MSVC 环境、主仓与独立夹具的复跑方式、可选测试及历史日志保护 |
| [MCP 协议](../src/mcp/PROTOCOL.md) | 17 个工具、输入输出、宿主授权、预算、缓存与取消限制 |
| [附件契约](native-attachment-contract.md) | 图片、文件、转发记录的严格定位和媒体发布规则 |
| [账号级密钥提供器](account-key-provider.md) | Windows 账号密钥捕获、DPAPI、构建依赖及安全边界 |

## 音频与媒体

| 文档 | 适用内容 |
| --- | --- |
| [音频核心](../src/toolkit/audio/README.md) / [MP3 批处理](../src/toolkit/audio/BATCH.md) | 原生 SILK 解码与 FFmpeg 编码、输入预算和逐项失败 |
| [本地 ASR](../src/toolkit/asr/LOCAL.md) / [云 ASR](../src/toolkit/asr/OPENAI.md) | whisper.cpp、Python 配置兼容及云授权差异 |
| [数据库语音关联](../src/toolkit/asr/DATABASE_MEDIA.md) | 精确消息和媒体身份、离线快照与账号绑定宿主的差异 |
| [转录缓存](../src/toolkit/asr/CACHE.md) / [回写](../src/toolkit/asr/WRITEBACK.md) | 缓存键、receipt、发布与非事务边界 |
| [SNS WASM](../src/toolkit/sns/VIDEO_RUNTIME.md) | Rust WASM、图片与视频限额、流式解码及播放验证边界 |

## 历史与参考

- [旧工作流缺口审计](legacy-workflow-gap-audit.md)：保留当时 G01-G12 的证据，当前映射以该文顶部及迁移记录为准，不是待办清单。
- [旧源码能力清单](legacy-capability-inventory.json)：38 个生产模块、544 个符号的静态起点，不是迁移完成率或用例覆盖率。
- [Windows 微信 4.1 验证记录](windows-wechat-4.1-key-provider-verification.md)：指定版本及当时构建的实机记录，不代表当前构建重做过实机验收。
- `tests/fixtures/` 内说明与审计日志：运行入口可随代码修正，但已发生的失败、测试数量和基线不改写为新结果。
- `vendor/wechat-decrypt/` 内 README、USAGE、EXE_USAGE、格式说明和 WASM 文档：随保留的第三方参考代码解释其原始行为，不作为 `wx toolkit` 当前安装或配置指南。不要按旧 Python/Node 启动命令推断原生入口仍调用脚本。
- [第三方声明](../THIRD_PARTY_NOTICES.md)、仓库及 vendor 许可证保留原文，不作为功能或发布状态说明。

## 维护约定

新增或修改公开入口时，同步 README、Agent 速查、对应协议/模块文档与当前架构图。历史报告只增加明确的时点和后续映射，不篡改原始测试结论。示例只用合成账号与占位路径；不记录密钥、私人聊天和当前机器运行身份。

当前只支持 Windows x64 MSVC。开发与自动化回归完成不等于所有模型、GPU、真实账号、任意媒体或安装发布包都已验收。企业微信排除本次移植验收，既有代码与文档保留，不以此宣称企微完整性。
