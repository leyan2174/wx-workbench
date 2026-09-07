# 数据库语音批量导出

## 当前入口（2026-09-07）

[audio/mod.rs](mod.rs) 已注册 `batch`，[toolkit CLI](../../cli/toolkit.rs) 已直接调用 `BatchOptions::from_config_file` 和 `convert_database`，无需再接线。

```powershell
wx toolkit voice-batch --config C:\account-workspace\config.json
wx toolkit voice-batch --config C:\account-workspace\config.json --output-dir C:\account-exports\voices --contacts wxid_a,wxid_b
```

上例是用法说明，本轮没有执行。`--config` 必填；`--output-dir` 和 `--contacts` 可选。
相对 `--config` 及显式 `--output-dir` 按进程工作目录解析；配置内部相对路径则按配置父目录解析，二者不能混淆。
`--contacts` 覆盖配置构造器读取的 `WECHAT_EXPORT_CONTACTS`，显式空字符串表示不筛选。
CLI 打印完整 JSON 报告；若 `failed > 0`，随后返回失败，已成功写出的文件保留，不是整批事务。
没有 CLI `--ffmpeg` 参数，程序调用方可直接设置 `BatchOptions.ffmpeg`。

| 状态 | 证据与边界 |
| --- | --- |
| 当前主线自动化 | Rust 精简后全量 `1325/0/11 ignored`、个人 Web `61/0`；8 次可选测试执行另行通过，MSVC check 仍有 9 警告。计数来自主线，不是本模块独立测试数 |
| 当前批量职责 | 只读取显式配置对应的已解密 `message/media_0.db`；不自动跨媒体分片、不解密数据库、不执行 ASR |
| 本轮验证范围 | 只静态校订文档，未运行 Cargo、ffmpeg 或真实账号；真实录音、模型/GPU、真实云服务未据此验收。企微排除目标，保留其既有实现和入口 |

当前证据出处见 [Rust 迁移记录](../../../docs/rust-migration.md)、[系统架构](../../../docs/architecture.md)；单文件差异见 [README.md](README.md)。

主线本轮文档同步验证：`C:/CodexLocal/wx-cli-doc-sync-tests.log` 终态退出 0，20 套件 `1325/0/11`；check 退出 0、9 警告，18 项 EXE help 检查通过。不是本文件维护者重跑；help 检查不替代真实批量导出或部署验收。

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

## 历史独立验证

以下保留 2026-09-07 集成前的独立 harness 记录，不是本轮重跑；当前主线验收见开头状态表。

独立项目位于 C:/CodexLocal/audio-validation-native，不运行全仓 Cargo，不使用主仓 target。
batch_tests.rs 使用 tempfile 内新建的真实 SQLite 数据库，不读取私人数据库或配置。
2026-09-07 最终 Windows x64 MSVC 独立 cargo check 成功，无警告；
显式启用 ffmpeg 测试后共 19 passed、0 failed、0 ignored（单文件 7 项，batch 12 项）。

验证用例覆盖配置路径推导、筛选语义、安全目录名、Name2Id 映射、unknown 回退、.info 内容和只写一次、
同名联系人隔离、联系人库缺失、主库缺失、源目录边界、已有文件跳过、坏 BLOB、无效字段、缺失编码器及临时文件清理。
另覆盖全点号/截断为空的目录名、Windows 设备名、编码期间目标晚出现时的不覆盖发布及目标为目录时的失败清理。

显式启用的端到端测试真实调用 ffmpeg，输入为已通过 pilk PCM 差分验证的合成 multi100.silk：

- 5 条 SQLite 记录首次：成功 4，失败 1；其中转换 3、已有跳过 1。
- 第二次：成功 4，失败 1；其中转换 0、已有跳过 4。
- media.db 和 contact.db 前后逐字节一致，既有 MP3 保留，无残留临时文件。

```powershell
cargo test --manifest-path C:\CodexLocal\audio-validation-native\Cargo.toml --target x86_64-pc-windows-msvc --config 'env.LIBCLANG_PATH="C:/CodexLocal/build-tools/libclang/clang/native"' --config 'env.WX_AUDIO_FIXTURES="C:/CodexLocal/src/wx-cli/tests/fixtures/audio"' -- --include-ignored --nocapture
```

默认测试仍忽略需外部 ffmpeg 的用例；显式可选执行与默认全量的 ignored 数分别报告。后续主线已经有集成记录，不能再用本节“独立验证”状态推定主仓未接入或未回归。
