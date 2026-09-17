# 文档导航

本文档描述 wx-workbench 的当前功能、契约与运行边界。原创文档由 AI 生成、整理和撰写，不是人类手工撰写；第三方资料及许可证保持原有归属。

## 使用与执行

- [工作流与前置条件](workflow-requirements.md)
- [账号密钥](account-key-provider.md)、[密钥存储](key-store.md)
- [请求契约](request-contracts.md)、[通信与导出](communication-and-export.md)
- [daemon 入口](daemon-entrypoints.md)、[后台任务](daemon-tasks.md)、[业务结果与进程管理](business-and-process.md)
- [名称与运行约定](project-naming.md)

## 架构与业务

- [系统架构](architecture.md)、[架构图](diagrams/README.md)
- [联系人](business-contacts.md)、[消息](business-messages.md)、[结构化消息](structured-message-boundary.md)、[收藏](favorites-boundary.md)
- [归档](archive-boundary.md)、[媒体](media-boundaries.md)、[文件与记录附件](native-attachment-contract.md)
- [图片发布](image-publication-boundary.md)、[严格 MCP 媒体](strict-media-host-boundary.md)
- [表情格式](emoticon-format.md)、[SNS 缓存](sns-cache-boundary.md)、[SNS 密钥流](../src/adapters/wechat/media/SNS_KEYSTREAM.md)
- [语音目录](voice-catalog-boundary.md)、[原始语音导出](../src/business/VOICE_EXPORT.md)

## 开发与来源

- [测试要求](testing-plan.md)、[测试入口](../tests/README.md)、[质量检查](quality-checks.md)、[编译告警](compiler-warnings.md)
- [文档维护](documentation-consistency.md)
- [第三方来源与许可](../THIRD_PARTY_NOTICES.md)、[分发边界](dmca-and-publication-risk.md)
