# 表情目录映射契约

参考来源是 vendor/wechat-decrypt/emoticons.py 的 build_emoji_lookup。oracle.py 通过 AST 提取映射逻辑，只运行合成只读 SQLite 输入；生产实现不调用 Python。

## 映射规则

- NonStore 保留首次插入顺序，重复值由最后一行覆盖，允许 NULL 和空字段。
- 模板收集包含没有 MD5 的行，最后一个非空模板生效。
- Store 跳过空或已存在 MD5，包名区分大小写；要求包含 &，不要求正则一定匹配。
- 小写十六进制正则保留部分、非参数及多次匹配；替换字符串按参考实现解释转义、八进制和整段匹配引用，非法引用即使没有匹配也报错。
- 标题只选 default 语言，重复项后值覆盖；NULL 转为空标题，与缺失标题区分。
- 计数以不同 NonStore MD5 和新增 Store MD5 为准。

## 边界

固定账号 DbCache 负责解密、缓存和 WAL；只选择精确规范相对库名，调用 get 时保留原始键，规范名冲突拒绝。缺密钥或源可返回空目录，解密和读取失败则返回脱敏错误。

仅缺失标题表是可选情况；坏 schema、视图、锁或非法行导致整次失败。字段只接受文本或 NULL，不兼容数值/BLOB 字典值。三个查询共用读事务。

公开错误的 Display、Debug 及测试断言不得输出含签名 URL 或密钥的完整映射。

## 运行

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --target x86_64-pc-windows-msvc --bin wx toolkit::emoticons::catalog
```

合成回归覆盖映射、替换表达式、独立账号密钥、冷缓存、WAL 增量、缓存命中及重启后更新；加载前后检查源 DB/WAL 字节不变。该过滤器不是全仓测试。
