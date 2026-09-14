# 数据库语音批量导出

## 入口

[audio/mod.rs](mod.rs) 已注册 `batch`，[toolkit CLI](../../cli/toolkit.rs) 已直接调用 `BatchOptions::from_config_file` 和 `convert_database`，无需再接线。

```powershell
wx toolkit voice-batch --config config.json
wx toolkit voice-batch --config C:\account-workspace\config.json --output-dir exports\voices --contacts synthetic-user-a,synthetic-user-b
```

`--config` 必填；`--output-dir` 和 `--contacts` 可选。
相对 `--config` 及显式 `--output-dir` 按进程工作目录解析；配置内部相对路径则按配置父目录解析，二者不能混淆。
`--contacts` 覆盖配置构造器读取的 `WECHAT_EXPORT_CONTACTS`，显式空字符串表示不筛选。
CLI 打印完整 JSON 报告；若 `failed > 0`，随后返回失败，已成功写出的文件保留，不是整批事务。
没有 CLI `--ffmpeg` 参数，程序调用方可直接设置 `BatchOptions.ffmpeg`。


库接口：

```rust
impl BatchOptions {
    pub fn from_config_file(path: &Path) -> anyhow::Result<Self>;
    pub fn from_config(config: &serde_json::Value, base: &Path) -> anyhow::Result<Self>;
}
pub fn convert_database(options: &BatchOptions) -> anyhow::Result<BatchReport>;
pub fn parse_contact_filter(raw: &str) -> Option<BTreeSet<String>>;
pub fn safe_dirname(name: &str) -> String;
```

BatchOptions 的字段全部公开，主程序可以直接构建，不必再次读取配置或全局环境：

```rust
BatchOptions {
    media_db,      // PathBuf: decrypted/message/media_0.db
    contact_db,    // PathBuf: decrypted/contact/contact.db
    output_dir,    // PathBuf: 联系人导出根目录
    contacts,      // Option<BTreeSet<String>>: 精确用户名筛选
    ffmpeg,        // PathBuf: 默认 ffmpeg，允许绝对路径
}
```

示例：

```rust
let options = audio::batch::BatchOptions::from_config_file(&config_path)?;
let report = audio::batch::convert_database(&options)?;
println!("{}", serde_json::to_string(&report)?);
```

复用主仓 rusqlite、chrono、md5、serde、serde_json、anyhow 以及单文件核心的 tempfile。
没有新增 batch 专用生产依赖，不需要 Python/pilk。

## 行为

- 配置 decrypted_dir 默认 decrypted，读取其 message/media_0.db 和 contact/contact.db。
- 配置中的相对路径以传入 base 或配置文件父目录为基准；显式 output_base_dir 优先。
- 未配置 output_base_dir 时，按旧脚本规则从 db_dir 的账号目录推导 base/wechat_files/账号目录名。
- 不自动扫描账号或改写配置；环境变量展开应由主程序的配置层完成。
- 配置构造器读取一次 WECHAT_EXPORT_CONTACTS：整体 strip 后 split(',')，与旧脚本相同；不按昵称匹配、不额外 trim 每个字段。
- Name2Id 的 rowid 对应 VoiceInfo.chat_name_id；无映射时使用 unknown_<id>。
- VoiceInfo 按 chat_name_id、create_time 顺序逐行读取，避免把全库 BLOB 一次载入内存。
- 联系人优先 remark > nick_name > username；读取联系人库失败时发出 warnings，继续以用户名导出。
- 输出 display/voice/YYYYmmdd_HHMMSS_local_id.mp3，使用本地时区，与旧 datetime.fromtimestamp 一致。
- .info 保留 username、alias、nick_name、remark 四行，只创建一次，不覆盖用户已有信息。
- 已有普通 MP3 文件直接跳过，仍计入 success，且不再尝试解码该行。
- 单条损坏记录、非法时间戳、转码失败累计 failed/failures 后继续。
- 主库无法打开或表结构不符返回整体 Err，不静默创建空库。
- 报告字段：total、success、failed、converted、skipped_existing、filtered、warnings、failures。
- 计数关系：success = converted + skipped_existing；完整返回时 total = success + failed + filtered。
- failures 每项包含可用的 chat_name_id、local_id 及错误，不包含语音 BLOB。

## 安全增强及边界

目录名替换旧非法字符，并处理控制字符、尾点、Windows 保留设备名和超长名称。
同一显示名属于不同 username 时，用 username 的稳定 MD5 短后缀分目录，避免旧脚本的跨联系人误跳过。
.info 第一行作为既有目录的联系人身份检查；原子、不覆盖地发布 .info。
输出必须位于解密数据库树外；拒绝输出目录及目标路径中的符号链接、Windows 重解析点。
源 SQLite 只读打开；转码使用既有隐藏 ffmpeg、同目录临时文件及原子发布实现。
批量转换先输出到独占暂存路径，再以 persist_noclobber 发布最终 MP3；
即使其他导出者在编码期间创建了目标，也不会覆盖，而是计入 skipped_existing。
发布失败或目标晚出现时会清理暂存文件。

保留单文件核心限制：最大 16 MiB/6000 个 SILK 包，非流式解码，ffmpeg 无超时机制。
本核心仅覆盖旧 voice_to_mp3.py 的 media_0.db 批量职责，不含跨 media 分片自动发现或 ASR。
Web 与 `wx tasks` 的任务由 [daemon 任务服务](../../daemon/tasks/mod.rs) 和[类型化计划](../../service/plan.rs) 编排，daemon 持有工作进程生命周期；音频核心没有逐条 GUI 回调。
未保证多个 batch 进程并发运行时的 exactly-once 语义；主程序应避免对同一输出目录并发启动。

## 测试

`batch_tests.rs` 使用临时 SQLite 数据库，覆盖路径推导、联系人筛选、Name2Id 映射、安全目录名、同名联系人隔离、已有项跳过、源库保护、坏 BLOB、缺编码器和临时文件清理。发布竞争测试要求晚出现的目标不被覆盖。

需要 FFmpeg 的端到端用例默认忽略，确认依赖后定向执行。测试命令见[测试说明](../../../tests/README.md)；本核心只处理配置指定的 `media_0.db`，不能用这些用例证明其他媒体分片均已导出。
