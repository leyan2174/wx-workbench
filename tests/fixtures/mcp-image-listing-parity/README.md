# 图片列表元数据测试

生产 helper 与 query 共用消息筛选、资源读取和目录扫描实现；独立 oracle 用于核对参考行为及明确差异。

## 行为

- `q_attachments` 提供轻量默认行为和返回字段；`q_attachments_with_image_metadata` 接受相同参数并提供图片元数据，二者复用单一消息筛选、全局排序和分页实现。
- 分页前保留 raw local_type 和分片来源。分页后对页面身份在**全部已解析分片**执行绑定参数的精确计数，每个身份每片最多匹配两行；同片重复和其他片分页截断之外的重复均为 `message_ambiguous`，不会根据有大量同时间戳就把唯一身份误报为歧义。
- `native_image::ResourceReader` 共享导出原有的精确关联 SQL/校验，一个页面一个只读资源事务。标准文件名扫描共享 `scan_candidates`，一页一次有界目录扫描；列表对所有月份、所有变体的多候选统一返回 size 歧义，导出的原有 full/h/t 优先规则未改变。
- `attachment::image_metadata::read_page` 不调用密钥提供器、decoder 或发布函数，不读取 DAT 正文。size 来自 `Pin` 固定文件句柄的 metadata 长度；原始加密 DAT 可以是零字节或超过解码器的大小限制。
- 行 schema：`md5: string|null`、`size: u64|null`、`resource_status`、`size_status`、`size_kind="encrypted_dat_metadata"`、`binding="exact_resource_standard_filename_metadata"`。严格满足 found iff md5Some、available iff sizeSome、无 md5 不允许 sizeSome；缺失 MD5、歧义消息和未请求大小时 size_status 为 not_requested。不返回路径、packed_info 或密钥。
- 坏资源 schema、侧车文件和读取错误返回失败，不冒充 missing。每页最多 1000 行；目录、候选和资源大小沿用共享防护的有界限制。唯一性检查针对已解析分片，不声明覆盖未知分片或抵御所有同权限并发篡改。

## 元数据与写入

MD5 来自精确匹配资源行的 packed_info，不是 username 的散列或明文图片摘要。size 是已固定常规 DAT 文件句柄的长度，不读取 DAT 正文；未知不能填零，真实零字节可以为零。标准候选只有 <md5>.dat、<md5>_h.dat、<md5>_t.dat，多候选以歧义状态表达。

账号查询允许正常 DbCache 缓存生成、WAL 更新和重解密。资源缓存通过 SQLite backup 转成私有临时静态快照，再交给严格资源读取器；不删除源 WAL 或更改源日志模式。只读列表不创建媒体输出、不解码、不获取图片密钥、不上传，但不能称为整个调用零磁盘写入。

参考实现的宽泛文件名前缀、任取首个候选、KB 舍入及提示文案不属于逐字兼容保证。资源关联仍不是数字签名，不保证同权限恶意进程的所有并发行为。

## 运行

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --bin wx mcp_image -- --nocapture
cargo test --manifest-path tests/fixtures/mcp-image-listing-parity/Cargo.toml -- --nocapture
cargo test --bin wx image_metadata -- --nocapture
```

生成器通过 AST 提取参考工具和解析函数，使用临时合成数据库与候选文件，不导入旧服务。audit hook 拒绝读取 DAT 正文，并检查输入摘要及未创建输出目录。普通核对使用 --check，不重建 golden。

回归覆盖同号复用、精确时间、raw type、跨片及同片重复、页截断之外的重复、空页、缺资源、零字节、多变体、坏 schema 和 sidecar。生产 query 测试另覆盖冷缓存、过期重解密及重复暖查询。
