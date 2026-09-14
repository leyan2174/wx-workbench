# MCP 图片查询适配


## API

宿主应调用以下入口，低层 q_decode_image 仍供已准备好密钥材料的内部调用者使用：

```rust
pub async fn q_decode_image_with_key_file(
    db: &DbCache, names: &Names, chat: &str,
    local_id: i64, create_time: i64,
    output_root: &Path, key_file: Option<&Path>,
) -> anyhow::Result<serde_json::Value>;
```

```rust
pub async fn q_decode_image(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    output_root: &Path,
    key: V2KeyMaterial<'_>,
) -> anyhow::Result<serde_json::Value>;
```

先调用真实 `strict_message::locate`，唯一性判断先于图片类型过滤。`create_time=0` 只表示消息定位时不筛时间，导出时仍传命中消息的真实时间。`kind` 保留全部高位，只有 base_type=3 才进入图片核心。

成功返回 `{"exit_code":0,"status":"published","image":ImageOutput}`，ImageOutput 包含完整消息身份、实际输出路径、源 DAT 路径和校验证据。聊天或消息不存在返回 exit_code=1；聊天或消息身份歧义返回 exit_code=2。清单、缓存、资源 schema、解码或发布错误通过 Result 传播。

仅在 daemon IPC 的 `DecodeImage` 分发边界，上述 `Err` 转为传输成功的静态业务响应 `{"exit_code":3,"status":"error","message":"Image export failed"}`，不返回原始错误链、路径或密钥材料；明确的消息缺失仍为 1，身份歧义仍为 2。`q_decode_image*` 的 `Result` 接口不变，这不是全部 CLI 错误的统一约定。

## 宿主依赖

- daemon 执行真实查询与宿主策略，CLI 负责宿主参数封送和认证请求。fixture 使用真实注册，不补写替代接口。
- 宿主传入固定账号的 DbCache/Names；附件根唯一来源是 `db.db_dir().parent()/msg/attach`。
- 宿主必须显式提供 output_root 和 V2KeyMaterial；适配器不读取配置，不使用旧 q_extract，不调用自动 provider、网络或 ffmpeg。阻塞任务持有的 AES 副本在结束时清零。
- 使用现有 DbCache 只读 `raw_db_keys()` 全键接口：适配器规范化斜线和大小写精确筛选 `message/message_resource.db`，保留原始键、不去重。拒绝零个或多个结果，再以原始键调用 db.get，不新增资源专用接口。
- `DbCache::output_protection_paths()` 返回 source、缓存根、mtime、已有解密产物，以及初始化上下文中的 config/keys/decrypted 路径。缓存元数据锁占用时拒绝返回部分清单；查询时不重新加载配置。内部 with_dirs 构造的合成缓存没有外部配置文件，真实 new 构造保留初始化上下文路径。


## 显式 Key 文件

宿主输出目录必须已存在且为绝对本机路径；空值、相对路径、UNC/设备路径先于账号和 key 文件访问失败。支持 Windows 本机规范盘符路径。source/cache 等目录不能与输出目录双向重叠；config、keys、mtime 和 image key 等保护文件不能位于输出目录内。目录守卫持有到导出结束。

image key 文件必须为显式绝对路径的常规文件，拒绝 reparse/symlink 和目录，最多 4 KiB，读取最多上限加一个探针字节；只读句柄拒绝并发写入/替换。文件缓冲区、已解析的字符串和密钥材料使用零化包装；错误不携带 key 值、key 文件名或原始错误链。

```json
{"aes_key":"1234567890abcdef","xor_key":"0xa2"}
```

字段为可选的 aes_key 字符串和 xor_key 整数或字符串，拒绝重复字段与未知字段。AES 完全沿用现有 CLI 规则：至少 16 个 ASCII 字节，取前 16 字节，不把字符串自动视为十六进制密钥。XOR 支持 0..255 整数、十进制字符串或 0x/0X 十六进制字符串，默认 0x88。没有 key 文件时仍允许 legacy/V1；V2 缺少 AES key 会失败，不尝试自动获取。

## 写入边界

加载资源缓存前捕获当前账号的消息与资源源文件清单；限定只使用 message/message_resource.db，拒绝额外 message_*resource*.db、源缺失、目录冒充、重解析点和未知消息分片。缓存加载后，在最终阻塞任务中重做完整性检查，并比较源文件身份、大小、修改时间及 WAL 的存在/属性。

资源缓存通过 `daemon/cache/snapshot.rs` 的 `ResourceSnapshot` 生成私有临时副本：使用 SQLite backup 读取包含 WAL 的逻辑视图，仅将副本转为 DELETE 日志模式，再交给静态资源读取器。复制前后核对源文件身份、大小和修改时间，失败或作用域结束清理临时目录；不删除源 WAL 或修改源日志模式。

所有清单复核都发生在 native_image 导出之前。导出核心负责有界读取、静态缓存资源检查、附件路径固定以及无覆盖发布。成功发布后只构造响应，不再做路径打开或清单检查，以免已生成图片却因可预见后置错误丢失成功记录。

清单捕获是观察到的账号快照，不是整个微信目录的全局事务锁，不承诺对抗特权进程伪造文件时间。宿主必须持续等待已派发的阻塞任务并保存成功响应；进程崩溃或取消 await 无法提供持久响应日志保证。本适配器不另建持久输出索引。

native_image 的 Windows、静态资源库、可信输出根、MD5 弱绑定及 wxgf 不转码等限制仍然适用。

## 测试

按[测试说明](../../README.md)准备依赖，从仓库根目录运行：

```powershell
cargo test --bin wx daemon::query::mcp_image::tests -- --nocapture
```

测试使用真实 DbCache、严格消息定位、图片核心和解码器，以合成数据覆盖原始键大小写与分隔符、重复别名、未知或缺失资源、类型与时间、歧义、损坏资源、显式 V2 密钥、清单变化及不覆盖发布。资源快照另有 WAL 内容读取和源文件保护测试。

run.ps1 委托 tests/run-module.ps1，使用根 Cargo.toml、Windows MSVC 和 wx 测试目标，不复制源码。可显式指定 TargetDir 或使用 Check 模式。真实图片内容和版本兼容性须另行验证。
