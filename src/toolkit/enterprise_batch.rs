//! 企业微信批量编排；只在显式调用时访问所选账号，不修改配置或现有密钥文件。
//! 密钥不进入报告；解密继续使用 enterprise 的离线主库/WAL 边界。

pub mod scan;
mod keys;
#[cfg(windows)]
pub mod windows;

use super::enterprise::{self, queries::{ExportFormat, OfflineStore}, DatabaseFormat, PAGE_SIZE};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs::{self, File}, io::{Read, Write}, path::{Component, Path, PathBuf}};
use zeroize::Zeroizing;

pub use keys::{KeyOptions, KeyRing};
#[cfg(test)]
pub use scan::ProcessMemory;
pub use scan::{ProcessScanner, ScanOptions, ScanReport};

#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub directory: PathBuf,
    pub account_id: String,
}

impl Account {
    /// 调用者必须选择一个账号 Data 或单账号快照，不能传入 WXWork 多账号父目录。
    pub fn open(directory: &Path) -> Result<Self> {
        let directory = checked_directory(directory)?;
        // 防止把 WXWork/<账号>/Data 树误当成一个账号递归解密。
        for child in fs::read_dir(&directory)? {
            let child = child?;
            if child.file_type()?.is_dir() {
                ensure!(!child.path().join("Data").is_dir(), "请选择单一账号 Data 目录，不得合并多个账号");
            }
        }
        let account_id = format!("account-{}", path_digest(&directory));
        Ok(Self { directory, account_id })
    }
}

/// 仅发现候选目录，不选择最近账号、不读数据库或进程、不改写配置。
pub fn discover_accounts(root: Option<&Path>) -> Result<Vec<Account>> {
    let root = match root {
        Some(root) => root.to_path_buf(),
        None => dirs::document_dir().context("无法定位 Documents；请指定 --root")?.join("WXWork"),
    };
    let root = checked_directory(&root)?;
    let mut accounts = Vec::new();
    for item in fs::read_dir(root)? {
        let item = item?;
        if is_reparse(&fs::symlink_metadata(item.path())?) || !item.file_type()?.is_dir() { continue; }
        let data = item.path().join("Data");
        match fs::symlink_metadata(&data) {
            Ok(meta) if meta.is_dir() && !is_reparse(&meta) => accounts.push(Account::open(&data)?),
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }
    }
    accounts.sort_by(|a, b| a.directory.cmp(&b.directory));
    Ok(accounts)
}

#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub item: String,
    pub stage: &'static str,
    pub code: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct DatabaseResult {
    pub database: String,
    pub output: PathBuf,
    pub format: &'static str,
    pub pages: u32,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct DecryptBatchReport {
    pub account: Account,
    pub output: PathBuf,
    pub boundary: &'static str,
    pub discovered: usize,
    pub decrypted: usize,
    pub copied: usize,
    pub failed: usize,
    pub databases: Vec<DatabaseResult>,
    pub failures: Vec<Failure>,
    pub scan: Option<ScanReport>,
}

#[derive(Debug, Serialize)]
pub struct ExportFile {
    pub conversation_id: String,
    pub format: &'static str,
    pub message_count: usize,
    pub output: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct ExportBatchReport {
    pub account: Account,
    pub output: PathBuf,
    pub selected_conversations: usize,
    pub exported_conversations: usize,
    pub partial_conversations: usize,
    /// 每会话去重后计一次；多格式不重复累计。
    pub message_count: usize,
    pub failed: usize,
    pub files: Vec<ExportFile>,
    pub failures: Vec<Failure>,
}

pub struct ExportOptions {
    pub conversations: Vec<String>,
    pub formats: Vec<ExportFormat>,
    pub self_id: Option<i64>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { conversations: Vec::new(), formats: vec![ExportFormat::Csv], self_id: None }
    }
}

impl ExportOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.formats.is_empty(), "至少选择一种导出格式");
        ensure!(self.conversations.iter().all(|s| !s.trim().is_empty()), "会话 ID 不得为空");
        Ok(())
    }
}

pub(crate) struct Database {
    relative: String,
    path: PathBuf,
    page: Zeroizing<Vec<u8>>,
    plain: bool,
}

pub(crate) fn inventory(account: &Account) -> Result<(Vec<Database>, Vec<Failure>)> {
    let mut databases = Vec::new();
    let mut failures = Vec::new();
    let mut pending = vec![account.directory.clone()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => { failures.push(failure(relative_name(account, &directory), "inventory", "directory_unreadable")); continue; }
        };
        for item in entries {
            let item = match item {
                Ok(item) => item,
                Err(_) => { failures.push(failure(relative_name(account, &directory), "inventory", "entry_unreadable")); continue; }
            };
            let path = item.path();
            let relative = relative_name(account, &path);
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => { failures.push(failure(relative, "inventory", "metadata_unreadable")); continue; }
            };
            if is_reparse(&meta) {
                failures.push(failure(relative, "inventory", "reparse_point_rejected"));
                continue;
            }
            if meta.is_dir() {
                if item.file_name() != "-journal" { pending.push(path); }
                continue;
            }
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("db")) { continue; }
            if !meta.is_file() || meta.len() == 0 || meta.len() % PAGE_SIZE as u64 != 0 {
                failures.push(failure(relative, "inventory", "unsupported_or_truncated_database"));
                continue;
            }
            let mut page = Zeroizing::new(vec![0; PAGE_SIZE]);
            if File::open(&path).and_then(|mut f| f.read_exact(&mut page)).is_err() {
                failures.push(failure(relative, "inventory", "page_one_unreadable"));
                continue;
            }
            let plain = page.starts_with(b"SQLite format 3\0");
            databases.push(Database { relative, path, page, plain });
        }
    }
    databases.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok((databases, failures))
}

pub fn decrypt_batch(
    source: &Path,
    output: &Path,
    options: &KeyOptions,
    scanner: Option<&dyn ProcessScanner>,
) -> Result<DecryptBatchReport> {
    options.validate()?;
    let account = Account::open(source)?;
    let (databases, failures) = inventory(&account)?;
    ensure!(!databases.is_empty() || !failures.is_empty(), "账号目录没有 .db 主库");
    // 先校验输出和授权，再获取密钥；参数错误不应触发进程扫描。
    output_location(&account, output)?;
    let (keys, scan) = keys::obtain(&account, &databases, options, scanner)?;
    let output = prepare_output(&account, output)?;
    decrypt_inventory(account, output, databases, failures, &keys, scan)
}

fn decrypt_inventory(account: Account, output: PathBuf, databases: Vec<Database>, failures: Vec<Failure>, keys: &KeyRing, scan: Option<ScanReport>) -> Result<DecryptBatchReport> {
    let snapshot = output.join(&account.account_id).join("decrypted");
    fs::create_dir_all(&snapshot)?;
    let mut report = DecryptBatchReport {
        account: account.clone(), output: snapshot.clone(), boundary: "offline_main_database_only_no_wal_replay",
        discovered: databases.len() + failures.len(), decrypted: 0, copied: 0,
        failed: 0, databases: Vec::new(), failures, scan,
    };
    let empty = Zeroizing::new([0u8; 16]);
    for database in &databases {
        let result = (|| -> Result<DatabaseResult> {
            // 再次检查目录身份，拒绝扫描后被替换为账号外路径的输入。
            ensure!(checked_directory(database.path.parent().context("Missing parent")?)?.starts_with(&account.directory), "账号目录已改变");
            ensure!(no_sidecars(&database.path)?, "主库存在 WAL/SHM/journal，需先提供一致离线快照");
            let key = if database.plain { &*empty } else { keys.get(&account, &database.relative).context("缺少已验证密钥")? };
            let destination = snapshot.join(relative_path(&database.relative)?);
            fs::create_dir_all(destination.parent().context("Missing output parent")?)?;
            checked_directory(destination.parent().context("Missing output parent")?)?;
            let result = enterprise::decrypt_database(&database.path, &destination, key)?;
            Ok(DatabaseResult {
                database: database.relative.clone(), output: destination,
                format: match result.format {
                    DatabaseFormat::PlainSqlite => "sqlite",
                    DatabaseFormat::WxSqlite3Aes128Header => "wxsqlite3_aes128_header",
                    DatabaseFormat::WxSqlite3Aes128Legacy => "wxsqlite3_aes128_legacy",
                }, pages: result.pages, bytes: result.bytes,
            })
        })();
        match result {
            Ok(result) => {
                if result.format == "sqlite" { report.copied += 1; } else { report.decrypted += 1; }
                report.databases.push(result);
            }
            Err(_) => {
                let code = if !no_sidecars(&database.path).unwrap_or(false) { "offline_sidecar_boundary" }
                    else if !database.plain && keys.get(&account, &database.relative).is_none() { "missing_verified_key" }
                    else { "decrypt_or_publish_failed" };
                report.failures.push(failure(database.relative.clone(), "decrypt", code));
            }
        }
    }
    report.failed = report.failures.len();
    write_json_new(&output.join("decrypt-report.json"), &report)?;
    Ok(report)
}

/// 多会话、多格式按项继续；每个输出 writer 均复用离线核心的原子不覆盖发布。
pub fn export_batch(snapshot: &Path, output: &Path, options: &ExportOptions) -> Result<ExportBatchReport> {
    options.validate()?;
    let account = Account::open(snapshot)?;
    let store = OfflineStore::open(&account.directory, options.self_id)?;
    let conversations = store.conversations()?;
    let selected: BTreeSet<String> = if options.conversations.is_empty() {
        conversations.iter().map(|c| c.conversation_id.clone()).collect()
    } else { options.conversations.iter().cloned().collect() };
    let output = prepare_output(&account, output)?;
    let destination = output.join(&account.account_id).join("conversations");
    fs::create_dir_all(&destination)?;
    let mut report = ExportBatchReport {
        account, output: destination.clone(), selected_conversations: selected.len(),
        exported_conversations: 0, partial_conversations: 0, message_count: 0,
        failed: 0, files: Vec::new(), failures: Vec::new(),
    };
    let mut formats = Vec::new();
    for format in &options.formats {
        if !formats.contains(format) { formats.push(*format); }
    }
    for id in selected {
        let Some(conversation) = conversations.iter().find(|c| c.conversation_id == id) else {
            report.failures.push(failure(id, "export", "conversation_missing_or_empty"));
            continue;
        };
        // 展示名只用于可读前缀；完整 ID 摘要防止截断、保留名、大小写碰撞和路径穿越。
        let folder = destination.join(format!("conv-{}-{}", safe_label(&conversation.display_name), digest(id.as_bytes())));
        if fs::create_dir(&folder).is_err() || checked_directory(&folder).is_err() {
            report.failures.push(failure(id, "export", "conversation_directory_failed"));
            continue;
        }
        let mut exported = 0;
        let mut count = None;
        for format in &formats {
            let extension = format_name(*format);
            match store.export_conversation(&id, &folder.join(format!("messages.{extension}")), *format) {
                Ok(file) => {
                    count = Some(file.message_count);
                    exported += 1;
                    report.files.push(ExportFile { conversation_id: id.clone(), format: extension, message_count: file.message_count, output: file.output });
                }
                Err(_) => report.failures.push(failure(id.clone(), extension, "conversation_export_failed")),
            }
        }
        if let Some(message_count) = count {
            report.message_count += message_count;
            let metadata = serde_json::json!({"conversation": conversation, "message_count": message_count, "complete": exported == formats.len()});
            if write_json_new(&folder.join(".info"), &metadata).is_err() {
                report.failures.push(failure(id.clone(), "metadata", "conversation_metadata_failed"));
            }
        }
        if exported == formats.len() { report.exported_conversations += 1; }
        else if exported > 0 { report.partial_conversations += 1; }
    }
    report.failed = report.failures.len();
    write_json_new(&output.join("export-report.json"), &report)?;
    Ok(report)
}

pub fn list_conversations(snapshot: &Path, self_id: Option<i64>) -> Result<Vec<enterprise::queries::Conversation>> {
    let account = Account::open(snapshot)?;
    OfflineStore::open(&account.directory, self_id)?.conversations()
}

/// Run 在任何取钥动作前检查两个输出均为新的、相互隔离的目录。
pub fn validate_run_outputs(source: &Path, decrypted: &Path, exported: &Path) -> Result<()> {
    let account = Account::open(source)?;
    let decrypted = output_location(&account, decrypted)?;
    let exported = output_location(&account, exported)?;
    ensure!(!decrypted.starts_with(&exported) && !exported.starts_with(&decrypted), "解密和导出输出目录必须相互隔离");
    Ok(())
}

pub(crate) fn failure(item: String, stage: &'static str, code: &'static str) -> Failure { Failure { item, stage, code } }
pub(crate) fn relative_name(account: &Account, path: &Path) -> String {
    path.strip_prefix(&account.directory).unwrap_or(Path::new(".")).to_string_lossy().replace('\\', "/")
}
pub(crate) fn relative_path(name: &str) -> Result<PathBuf> {
    ensure!(!name.is_empty() && !name.contains(':') && !name.contains('\0'), "无效相对数据库路径");
    let normalized = name.replace('\\', "/");
    ensure!(normalized.split('/').all(|s| !s.is_empty() && s != "." && s != ".." && !s.ends_with([' ', '.'])), "无效相对数据库路径");
    let path = PathBuf::from(normalized);
    ensure!(path.components().all(|c| matches!(c, Component::Normal(_))), "数据库路径必须为相对路径");
    Ok(path)
}

pub(crate) fn is_reparse(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() { return true; }
    #[cfg(windows)]
    { use std::os::windows::fs::MetadataExt; meta.file_attributes() & 0x400 != 0 }
    #[cfg(not(windows))]
    { false }
}

pub(crate) fn checked_directory(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    for ancestor in absolute.ancestors() {
        let meta = fs::symlink_metadata(ancestor)?;
        ensure!(meta.is_dir() && !is_reparse(&meta), "目录必须存在且不得经过符号链接或重解析点");
    }
    Ok(fs::canonicalize(absolute)?)
}

pub(crate) fn no_sidecars(path: &Path) -> Result<bool> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_os_string(); name.push(suffix);
        match fs::symlink_metadata(Path::new(&name)) {
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

fn prepare_output(account: &Account, output: &Path) -> Result<PathBuf> {
    let output = output_location(account, output)?;
    fs::create_dir(&output).context("输出目录必须尚不存在；父目录必须已存在")?;
    checked_directory(&output)?;
    write_json_new(&output.join("account.json"), account)?;
    Ok(output)
}

fn output_location(account: &Account, output: &Path) -> Result<PathBuf> {
    let output = std::path::absolute(output)?;
    let parent = checked_directory(output.parent().context("输出须有父目录")?)?;
    let output = parent.join(output.file_name().context("输出须指定新目录名")?);
    ensure!(!output.starts_with(&account.directory) && !account.directory.starts_with(&output), "输出必须与源账号目录隔离");
    match fs::symlink_metadata(&output) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(e.into()),
        Ok(_) => anyhow::bail!("输出目录已存在；拒绝覆盖或合并账号数据"),
    }
    Ok(output)
}

fn path_digest(path: &Path) -> String {
    // Windows 路径用 UTF-16 原码位，避免有损转换混淆账号标识。
    #[cfg(windows)]
    { use std::os::windows::ffi::OsStrExt; digest(&path.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect::<Vec<_>>()) }
    #[cfg(not(windows))]
    { digest(path.as_os_str().as_encoded_bytes()) }
}
fn digest(value: &[u8]) -> String { format!("{:x}", Sha256::digest(value)) }
fn safe_label(value: &str) -> String {
    value.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(24).collect()
}
pub fn format_name(format: ExportFormat) -> &'static str {
    match format { ExportFormat::Json => "json", ExportFormat::Csv => "csv", ExportFormat::Html => "html" }
}

fn write_json_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = checked_directory(path.parent().context("缺少输出父目录")?)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(path).map_err(|_| anyhow::anyhow!("元数据不覆盖发布失败"))?;
    Ok(())
}
