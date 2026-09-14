# 文档索引

## 使用

- [项目与命令入口](../README.md)：安装、账号选择、查询、导出、MCP 和 Web。
- [账号密钥](account-key-provider.md)：provider、DPAPI、权限与显式重启。
- [工作流条件](legacy-workflow-gap-audit.md)：媒体、转录、下载、更新和清理的前置条件。
- [附件契约](native-attachment-contract.md)：消息定位、资源匹配与发布。
- [MCP 协议](../src/mcp/PROTOCOL.md)：工具参数、限额、会话与错误。

## 实现

- [架构](architecture.md)、[入口边界](daemon-entrypoints.md)、[后台任务](daemon-tasks.md)。
- [架构图](diagrams/README.md)：图源与生成约束。
- [音频](../src/toolkit/audio/README.md)与[批量音频](../src/toolkit/audio/BATCH.md)。
- [本地 ASR](../src/toolkit/asr/LOCAL.md)、[云端授权](../src/toolkit/asr/OPENAI.md)、[缓存](../src/toolkit/asr/CACHE.md)、[数据库音频](../src/toolkit/asr/DATABASE_MEDIA.md)、[回写](../src/toolkit/asr/WRITEBACK.md)。
- [SNS 视频](../src/toolkit/sns/VIDEO_RUNTIME.md)。

## 维护

- [测试说明](../tests/README.md)。
- [测试与整理计划](testing-plan.md)。
- [开发与回归验证](rust-migration.md)。
- [第三方来源与许可](../THIRD_PARTY_NOTICES.md)。

文档只说明当前接口与限制。私人账号样本、本机路径及每轮测试日志留在仓库外。
