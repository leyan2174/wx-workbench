# 附件元数据查询安全回归

夹具将合成消息交给真实严格定位与附件引用实现，检查不安全的元数据是否被错误降级为“未知”后继续查找。

嵌套元素、重复字段、畸形整数、负大小和无效 MD5 必须拒绝，不能忽略消息约束后返回同名错误附件。有效 MD5 要与候选真实内容匹配，无摘要唯一候选只能报告启发式关联。

回归同时核对完整分片身份、XML 边界、候选歧义和源文件字节不变。合成缓存容器不证明真实账号认证或解密；显式目录的账号来源由调用方保证。

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --offline --manifest-path tests/fixtures/native-attachment-security/Cargo.toml -- --nocapture
```

解析与扫描接口见[附件引用测试](../attachment-refs/README.md)，公共行为见[附件契约](../../../docs/native-attachment-contract.md)。
