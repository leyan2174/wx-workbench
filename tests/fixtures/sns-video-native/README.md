# SNS 视频 WASM 回归

本夹具验证受哈希约束的供应商 WASM ABI、密钥流和视频前缀解码。向量使用公开合成输入，常规 Rust 测试不需要 Node；重新生成向量才需要 Node 参考包装器。

通用密钥流上限为 25 MiB，供相册图片使用；视频仅 XOR 前 128 KiB 并保留尾部。燃料按对齐长度计算，受硬上限和调用方预算限制，不能通过提高输入限额绕过资源预算。

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --offline --manifest-path tests/fixtures/sns-video-native/Cargo.toml -- --nocapture
```

覆盖对齐边界、明文直通、后缀保留、错误头、未知模块、非法密钥、低燃料、低内存及异常后的独立调用。生成器会改写 golden，不应在普通回归中无意重建。ABI、预算和许可来源见[WASM 契约](../../../src/toolkit/sns/VIDEO_RUNTIME.md)。
