# ASR 缓存安全回归

本夹具直接引用生产 cache、cached、ASR、audio 及相关 CLI 模块，不复制生产逻辑。依赖设置与主项目一致，尤其保留 serde_json 的 arbitrary_precision。

build.rs 从相邻夹具准备合成程序和 SILK/PCM 数据，复制后核对字节；不修改公共 CARGO_MANIFEST_DIR 或其他测试。输入全部为临时合成文件，不使用真实账号、凭证、模型或云服务。

## 覆盖范围

- 失败不落盘、损坏缓存及账号封套拒绝、未知字段和高精度数值保留。
- 读取字节上限、打开后的非协作替换、硬链接原子发布、最终重解析点拒绝。
- 显式数据库与缓存路径隔离，拒绝数据库在目录外的硬链接别名。
- 成功命中、空成功、模型或程序内容变化、后端失败及持久化失败。
- receipt 记录的身份、配置、摘要及提交前拒绝，见[receipt 测试](../voice-cache-receipt/README.md)。

账号摘要是调用方命名隔离，不是认证或签名。原子替换不应改写硬链接别名的原字节；最终检查到 rename 仍有非协作竞态。64 MiB 是文件字节限制，不是内存峰值承诺。完整契约见[缓存说明](../../../src/toolkit/asr/CACHE.md)。

## 运行

按[测试说明](../../README.md)准备 MSVC 和 libclang 后，从仓库根目录执行：

```powershell
cargo test --offline --manifest-path tests/fixtures/asr-cache-security/Cargo.toml -- --nocapture
```

文件符号链接测试可能需要额外权限；默认跳过不计通过，目录 junction 覆盖也不能代替文件符号链接覆盖。run.ps1 只执行分组筛选测试，不能当作上述无过滤运行。
