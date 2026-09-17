# 原生 MCP 语音列表


接口：

```rust
pub async fn q_voice_messages(
    db: &DbCache, query: &VoiceQuery,
) -> anyhow::Result<Vec<VoiceMessage>>;
pub fn query_voice_shards(shards: &[MediaShard], query: &VoiceQuery)
    -> anyhow::Result<Vec<VoiceMessage>>;
pub fn resolve_exact_chat(chat: &str, names: &HashMap<String, String>)
    -> anyhow::Result<String>;
```

VoiceQuery 为显式 username、usize limit/offset、Option<i64> since/until。
区间两端均含；时间字符串由上层现有解析器处理，不在核心猜时区。
精确 username 直接传入；显示名必须精确唯一解析，不作子串匹配。

DbCache 提供账号级 `pub(crate) media_db_keys(&self) -> Vec<String>`，
查询内部调用 helper，不接受外部 media_keys 参数。
从已加载配置返回完整媒体原始键，不复用仅含消息片的 Names.msg_db_keys。
本模块不访问 all_keys、不读取任何密钥配置。适配器核对磁盘清单并解析全部
分片，任一未知/缺失/未解密/错误都返回 Err；不得用空列表当部分成功。

查询输出只含 username/source/chat_name_id/media_rowid/local_id/create_time/
voice_data_bytes（Option<u64>）。source 固定规范相对键 message/media_N.db，
不暴露解密绝对路径。仅 SELECT length/typeof，不读取、写出或解码音频。
排序为 create_time DESC、source ASC、local_id DESC、media_rowid DESC；
同号跨片不去重。重复 Name2Id 精确归属报错；NULL 长度保持 None，空 BLOB 为0。

READ_ONLY SQLite 每片独立读事务；缺片或模式损坏不跳过。全局排序稳定是
针对固定数据集，不保证多个库同时刻快照或并发新增消息下跨页不漂移。
显式离线入口不能自行证明清单账号归属/完整性，必须由调用方保证。

专属 harness 的 DbCache 是公开 API 形状替身，只验证 None/清单边界，不
伪称真实解密回归。SQLite 库全部由测试合成，不使用真实数据。

```powershell
cargo test --offline --manifest-path tests/fixtures/mcp-voice/Cargo.toml --target x86_64-pc-windows-msvc
```

依赖均已有 anyhow、rusqlite、serde、same-file，测试另用 tempfile/tokio；
生产依赖和 Windows feature 以根 Cargo.toml 为准。

## DbCache 与来源键

DbCache::with_dirs 原样保存 all_keys，get_with_mode 使用 all_keys.get(rel_key)
精确匹配，之后才把路径分隔符转换为本机格式。不能返回规范化键再调用 get。
只读 helper 筛选规则如下：

```rust
pub(crate) fn media_db_keys(&self) -> Vec<String> {
    let mut keys: Vec<_> = self.all_keys.keys().filter(|key| {
        let normalized = key.replace('\\', "/").to_ascii_lowercase();
        normalized.strip_prefix("message/media_")
            .and_then(|s| s.strip_suffix(".db"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }).cloned().collect();
    keys.sort();
    keys
}
```

返回原始键，不返回密钥值，不在 helper 内 get/解密，不按文件存在性过滤。
例如 message\\MEDIA_0.DB 可被选中且原样返回；排除 message_0.db、
media_cache.db、media_.db、media_0.db-wal、非 ASCII 数字和其他目录。
保留原始键的别名重复，由适配器拒绝重复 canonical source，不能静默 dedup。
适配器现以原始键调用 get，以小写正斜杠 source 表示证据。

生产兼容性：get 的 Err/None 均传播为整个请求失败；SQL 模式、类型、锁和
读取错误增加具体相对 source 上下文。READ_ONLY + 独立读事务兼容普通解密
SQLite；DbCache 自身仍可能更新解密缓存/WAL，这不是端到端零磁盘写入承诺。
SQLite 只读连接在某些 WAL 环境可能创建/使用 shm sidecar，不使用 immutable
跳过 WAL，避免静默读旧数据。多库不是统一快照，DbCache 刷新与 SQL 查询的
并发协调由 daemon 查询层负责。

旧 MCP 返回的是显示名标题、格式化时间、local_id、四舍五入 KB 和分页提示，
没有原生 JSON schema。本模块保留底层 create_time/local_id/真实字节长度并
新增归属证据；显示名、时区格式化、分页提示应由 MCP 展示层实现。
旧 NULL 长度可能导致格式化异常，本模块返回 None，不将未知长度伪装为零。

## rowid 模式校验

在每片同一读事务内，归属查询前使用 pragma_table_list 验证 Name2Id 和
VoiceInfo 为普通 rowid 表（非视图/虚拟表/WITHOUT ROWID）。使用 table_xinfo
检查所有列，包括生成列；任何大小写的 rowid/_rowid_/oid 用户列均拒绝，
不改用别名猜测。不使用正则解析建表语句。非保留名称的 INTEGER PRIMARY KEY
仍可作为真实 SQLite rowid 别名。

模式拒绝和跨联系人隔离见[安全回归](../mcp-voice-security/README.md)，环境与运行说明见[测试说明](../../README.md)。生产 IPC 与账号隔离另由[只读工具进程测试](../mcp-readonly-runtime/README.md)覆盖。
