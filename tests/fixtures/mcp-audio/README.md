# MCP 内部音频准备测试

夹具覆盖 daemon 的语音准备与证据绑定。媒体 local_id 与消息 local_id 不可互换；需要在完整来源清单内唯一关联联系人、server_id、时间和媒体行。

原始音频、准备结果的尺寸及摘要均有边界，畸形或冲突输入必须拒绝。内部 prepared_audio 只供 daemon 内宿主执行器使用，不通过公开 MCP text 返回音频字节。

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --offline --manifest-path tests/fixtures/mcp-audio/Cargo.toml -- --nocapture
```

此层不证明 WAV 发布、模型质量或上传授权；完整执行见[MCP 契约](../../../src/mcp/PROTOCOL.md)，关联规则见[数据库媒体说明](../../../src/toolkit/asr/DATABASE_MEDIA.md)。
