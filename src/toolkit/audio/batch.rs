//! 兼容旧 voice_to_mp3.py 的 media_0.db 批量导出，不读取加密库或私人配置。

use anyhow::{ensure, Context, Result};
use chrono::{Local, TimeZone};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct BatchOptions {
    pub media_db: PathBuf,
    pub contact_db: PathBuf,
    pub output_dir: PathBuf,
    pub contacts: Option<BTreeSet<String>>,
    pub ffmpeg: PathBuf,
}

impl BatchOptions {
    /// 相对路径以配置文件目录为基准；不扫描账号、不写回配置。
    pub fn from_config_file(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path).context("resolve voice batch config")?;
        let config = serde_json::from_slice(&fs::read(&path)?)?;
        Self::from_config(&config, path.parent().context("config has no parent")?)
    }

    /// 也可直接接收主程序已经解析的配置；显式 output_base_dir 优先。
    pub fn from_config(config: &serde_json::Value, base: &Path) -> Result<Self> {
        #[derive(Deserialize)]
        struct Config {
            decrypted_dir: Option<PathBuf>,
            output_base_dir: Option<PathBuf>,
            db_dir: Option<PathBuf>,
        }
        let config: Config =
            serde_json::from_value(config.clone()).context("invalid voice batch config")?;
        let base = std::path::absolute(base)?;
        let decrypted = resolve(
            &base,
            config
                .decrypted_dir
                .as_deref()
                .unwrap_or(Path::new("decrypted")),
        )?;
        let output_dir = if let Some(output) = config.output_base_dir {
            resolve(&base, &output)?
        } else {
            let db = resolve(
                &base,
                config
                    .db_dir
                    .as_deref()
                    .context("db_dir or output_base_dir is required")?,
            )?;
            let account = if db
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("db_storage"))
            {
                db.parent().context("db_dir has no account directory")?
            } else {
                &db
            };
            base.join("wechat_files").join(
                account
                    .file_name()
                    .context("cannot infer account output directory")?,
            )
        };
        Ok(Self {
            media_db: decrypted.join("message").join("media_0.db"),
            contact_db: decrypted.join("contact").join("contact.db"),
            output_dir,
            contacts: parse_contact_filter(
                &std::env::var("WECHAT_EXPORT_CONTACTS").unwrap_or_default(),
            ),
            ffmpeg: PathBuf::from("ffmpeg"),
        })
    }
}

/// 与旧脚本一致：整体去空白后按逗号分隔，不模糊匹配昵称。
pub fn parse_contact_filter(raw: &str) -> Option<BTreeSet<String>> {
    let raw = raw.trim();
    (!raw.is_empty()).then(|| raw.split(',').map(str::to_owned).collect())
}

#[derive(Debug, Default, Serialize)]
pub struct BatchReport {
    pub total: u64,
    pub success: u64,
    pub failed: u64,
    pub converted: u64,
    pub skipped_existing: u64,
    pub filtered: u64,
    pub warnings: Vec<String>,
    pub failures: Vec<BatchFailure>,
}

#[derive(Debug, Serialize)]
pub struct BatchFailure {
    pub chat_name_id: Option<i64>,
    pub local_id: Option<i64>,
    pub error: String,
}

#[derive(Debug, Default)]
struct Contact {
    alias: String,
    remark: String,
    nick_name: String,
}

/// 单条错误累计后继续；无法打开主库、缺表等结构错误整体返回 Err。
/// success 保持旧口径：converted + skipped_existing。
pub fn convert_database(options: &BatchOptions) -> Result<BatchReport> {
    // absolute 会折叠 ..；必须先验证原始输入，且不能在检查前创建任何目录。
    ensure!(
        !options
            .output_dir
            .components()
            .any(|part| part == Component::ParentDir),
        "parent components in output are not allowed"
    );
    let output = std::path::absolute(&options.output_dir)?;
    validate_output(options, &output)?;
    let connection = readonly(&options.media_db).context("open media database")?;
    let snapshot = connection.unchecked_transaction()?;
    let mut names = BTreeMap::new();
    let mut query = snapshot.prepare("SELECT rowid, user_name FROM Name2Id")?;
    for row in query.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
    })? {
        let (id, name) = row?;
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            names.insert(id, name);
        }
    }
    let mut report = BatchReport::default();
    let contacts = match load_contacts(&options.contact_db) {
        Ok(contacts) => contacts,
        Err(error) => {
            report.warnings.push(format!(
                "contact database unavailable; using usernames: {error:#}"
            ));
            BTreeMap::new()
        }
    };
    let mut query = snapshot.prepare(
        "SELECT chat_name_id, create_time, local_id, voice_data, length(voice_data) FROM VoiceInfo ORDER BY chat_name_id, create_time"
    )?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        report.total += 1;
        let chat_id = row.get::<_, i64>(0).ok();
        let local_id = row.get::<_, i64>(2).ok();
        let result = (|| -> Result<()> {
            let chat_id = chat_id.context("invalid chat_name_id")?;
            let username = names
                .get(&chat_id)
                .cloned()
                .unwrap_or_else(|| format!("unknown_{chat_id}"));
            if options
                .contacts
                .as_ref()
                .is_some_and(|filter| !filter.is_empty() && !filter.contains(&username))
            {
                report.filtered += 1;
                return Ok(());
            }
            ensure!(
                !username.chars().any(char::is_control),
                "username contains control characters"
            );
            let local_id = local_id.context("invalid local_id")?;
            let timestamp: i64 = row.get(1).context("invalid create_time")?;
            let date = Local
                .timestamp_opt(timestamp, 0)
                .single()
                .context("create_time out of range")?;
            let contact = contacts.get(&username);
            let display = contact
                .map(|c| {
                    if !c.remark.is_empty() {
                        c.remark.as_str()
                    } else {
                        c.nick_name.as_str()
                    }
                })
                .filter(|s| !s.is_empty())
                .unwrap_or(&username);
            let directory = contact_directory(&output, &username, display, contact)?;
            let voice = directory.join("voice");
            checked_mkdir(&voice)?;
            let target = voice.join(format!("{}_{local_id}.mp3", date.format("%Y%m%d_%H%M%S")));
            reject_reparse(&target)?;
            if target.exists() {
                ensure!(target.is_file(), "existing MP3 path is not a file");
                report.success += 1;
                report.skipped_existing += 1;
                return Ok(());
            }
            let size: Option<i64> = row.get(4)?;
            ensure!(
                size.is_some_and(|size| (1..=16 * 1024 * 1024).contains(&size)),
                "empty or oversized voice_data"
            );
            let data: Vec<u8> = row.get(3).context("voice_data is not a BLOB")?;
            // 每条只保留当前压缩数据；使用现有原子转换入口，不重复实现编解码。
            let mut source = tempfile::NamedTempFile::new_in(&voice)?;
            source.write_all(&data)?;
            source.flush()?;
            // 单文件入口允许替换；批量先转到独占暂存路径，最终发布必须不覆盖。
            let staged = tempfile::NamedTempFile::new_in(&voice)?.into_temp_path();
            super::convert_silk_to_mp3_with_ffmpeg(source.path(), &staged, &options.ffmpeg)?;
            if publish_mp3(staged, &target)? {
                report.converted += 1;
            } else {
                report.skipped_existing += 1;
            }
            report.success += 1;
            Ok(())
        })();
        if let Err(error) = result {
            report.failed += 1;
            report.failures.push(BatchFailure {
                chat_name_id: chat_id,
                local_id,
                error: format!("{error:#}"),
            });
        }
    }
    Ok(report)
}

// 返回 false 表示编码期间已有其他导出者发布目标，此时保留既有文件。
fn publish_mp3(staged: tempfile::TempPath, target: &Path) -> Result<bool> {
    match staged.persist_noclobber(target) {
        Ok(()) => Ok(true),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            reject_reparse(target)?;
            ensure!(target.is_file(), "existing MP3 path is not a file");
            Ok(false)
        }
        Err(error) => {
            Err(error.error).context("publish batch MP3 without replacing existing output")
        }
    }
}

fn readonly(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

fn load_contacts(path: &Path) -> Result<BTreeMap<String, Contact>> {
    let connection = readonly(path)?;
    let mut query = connection.prepare("SELECT username, alias, remark, nick_name FROM contact")?;
    let mut result = BTreeMap::new();
    for row in query.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Contact {
                alias: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                remark: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                nick_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            },
        ))
    })? {
        let (username, info) = row?;
        result.insert(username, info);
    }
    Ok(result)
}

fn resolve(base: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(!path.as_os_str().is_empty(), "empty configured path");
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    })
}

fn reject_reparse(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "reparse point is not allowed: {}",
                    path.display()
                );
            }
            ensure!(
                !metadata.file_type().is_symlink(),
                "symlink is not allowed: {}",
                path.display()
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn checked_mkdir(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        reject_reparse(ancestor)?;
    }
    fs::create_dir_all(path)?;
    reject_reparse(path)
}

fn validate_output(options: &BatchOptions, output: &Path) -> Result<()> {
    for ancestor in output.ancestors() {
        reject_reparse(ancestor)?;
    }
    // 禁止导出到解密库树内；保护整个 message/contact 目录，不仅是两个数据库文件。
    let mut existing = output;
    while !existing.exists() {
        existing = existing
            .parent()
            .context("cannot resolve output ancestor")?;
    }
    let resolved_output = existing
        .canonicalize()?
        .join(output.strip_prefix(existing)?);
    for database in [&options.media_db, &options.contact_db] {
        if let Some(root) = database
            .parent()
            .and_then(Path::parent)
            .filter(|root| root.exists())
        {
            let root = root.canonicalize()?;
            let root = root.to_string_lossy().to_lowercase();
            let target = resolved_output.to_string_lossy().to_lowercase();
            ensure!(
                target != root
                    && !target.starts_with(&format!(
                        "{}{}",
                        root.trim_end_matches(['\\', '/']),
                        std::path::MAIN_SEPARATOR
                    )),
                "voice output must be outside the decrypted database directory"
            );
        }
    }
    Ok(())
}

/// 保留旧非法字符替换，并覆盖 Windows 保留设备名、尾点及控制字符。
pub fn safe_dirname(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|ch| {
            if ch.is_control() || "\\/:*?\"<>|".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let name = name.trim().trim_end_matches('.').trim();
    let mut result: String = if name.is_empty() { "unknown" } else { name }
        .chars()
        .take(100)
        .collect();
    result = result.trim_end_matches(['.', ' ']).to_owned();
    if result.is_empty() {
        return "unknown".to_owned();
    }
    let stem = result
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let device_number = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || device_number.is_some_and(|number| {
            matches!(
                number,
                "1" | "2"
                    | "3"
                    | "4"
                    | "5"
                    | "6"
                    | "7"
                    | "8"
                    | "9"
                    | "\u{b9}"
                    | "\u{b2}"
                    | "\u{b3}"
            )
        })
    {
        result.insert(0, '_');
    }
    result
}

fn contact_directory(
    root: &Path,
    username: &str,
    display: &str,
    contact: Option<&Contact>,
) -> Result<PathBuf> {
    let base = safe_dirname(display);
    let digest = format!("{:x}", md5::compute(username.as_bytes()));
    for name in [base.clone(), format!("{base}~{}", &digest[..12])] {
        let directory = root.join(name);
        checked_mkdir(&directory)?;
        let info = directory.join(".info");
        reject_reparse(&info)?;
        if info.exists() {
            let existing = fs::read_to_string(&info)?;
            if existing.lines().next() == Some(format!("username:  {username}").as_str()) {
                return Ok(directory);
            }
            continue;
        }
        let empty = Contact::default();
        let contact = contact.unwrap_or(&empty);
        let clean = |text: &str| text.replace(['\r', '\n'], " ");
        let contents = format!(
            "username:  {username}\nalias:     {}\nnick_name: {}\nremark:    {}\n",
            clean(&contact.alias),
            clean(&contact.nick_name),
            clean(&contact.remark)
        );
        // 独占发布 .info，不能覆盖另一联系人已经取得的目录所有权。
        let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
        temporary.write_all(contents.as_bytes())?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(&info) {
            Ok(_) => return Ok(directory),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                // 同一联系人并发取得了目录时复用，不另建一个哈希后缀目录。
                reject_reparse(&info)?;
                if fs::read_to_string(&info)?.lines().next()
                    == Some(format!("username:  {username}").as_str())
                {
                    return Ok(directory);
                }
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    anyhow::bail!("contact output directory belongs to another username")
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
