//! 兼容旧 voice_to_mp3.py 的 media_0.db 批量导出，不读取加密库或私人配置。

use anyhow::{ensure, Context, Result};
use chrono::{Local, TimeZone};
#[cfg(test)]
use rusqlite::Connection;
use serde::Deserialize;
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

use crate::adapters::wechat::{
    contacts::batch::{self as contact_source, Contact},
    media::voice_export::BatchSource,
};
use crate::business::voice_export::BatchProgress;
use crate::business::voice_export::{batch_selected, BatchFailureStage, BatchItemOutcome};

#[derive(Debug, Default, serde::Serialize)]
pub struct BatchReport {
    #[serde(flatten)]
    pub progress: BatchProgress,
    pub warnings: Vec<String>,
    pub failures: Vec<BatchFailure>,
}

/// Legacy JSON diagnostic projection; these raw IDs do not establish strict identity.
#[derive(Debug, serde::Serialize)]
pub struct BatchFailure {
    pub chat_name_id: Option<i64>,
    pub local_id: Option<i64>,
    pub error: String,
}

fn failure_message(stage: BatchFailureStage) -> &'static str {
    match stage {
        BatchFailureStage::Metadata => "invalid voice metadata",
        BatchFailureStage::Directory => "voice output directory refused",
        BatchFailureStage::Material => "empty, invalid or oversized voice_data",
        BatchFailureStage::Conversion => "start ffmpeg or complete bounded audio conversion failed",
        BatchFailureStage::Publication => "voice publication refused",
    }
}

/// The wrapper retains the existing synchronous API. Worker cancellation is owned by its Job.
#[cfg(test)]
pub fn convert_database(options: &BatchOptions) -> Result<BatchReport> {
    convert_database_checked(options, &[], || false)
}

/// The caller owns cancellation and any additional protected account paths.
pub fn convert_database_checked(
    options: &BatchOptions,
    protected: &[PathBuf],
    mut cancelled: impl FnMut() -> bool,
) -> Result<BatchReport> {
    ensure!(!cancelled(), "Audio batch cancelled");
    ensure!(
        !options
            .output_dir
            .components()
            .any(|part| part == Component::ParentDir),
        "parent components in output are not allowed"
    );
    let output = std::path::absolute(&options.output_dir)?;
    validate_output(options, &output)?;
    let source = BatchSource::open(&options.media_db).context("open media database")?;
    let mut report = BatchReport::default();
    let contact_read = (|| -> Result<_> {
        let pin = crate::attachment::local_files::Pin::open(&options.contact_db, false)?;
        let contacts = contact_source::read(&options.contact_db)?;
        pin.verify()?;
        Ok((contacts, Some(pin)))
    })();
    let (contacts, contact_pin) = match contact_read {
        Ok(projection) => projection,
        Err(_) => {
            report
                .warnings
                .push("contact database unavailable; using usernames".into());
            (BTreeMap::new(), None)
        }
    };
    let mut protected = protected.to_vec();
    protected.extend([options.media_db.clone(), options.contact_db.clone()]);
    source.visit(|entry, material| {
        ensure!(!cancelled(), "Audio batch cancelled");
        let mut failure_stage = BatchFailureStage::Metadata;
        let result = (|| -> Result<BatchItemOutcome> {
            let username = entry.username.as_deref().context("invalid chat_name_id")?;
            if !batch_selected(username, options.contacts.as_ref()) {
                return Ok(BatchItemOutcome::Filtered);
            }
            ensure!(!username.chars().any(char::is_control), "invalid username");
            let local_id = entry.local_id.context("invalid local_id")?;
            let date = Local
                .timestamp_opt(entry.timestamp.context("invalid create_time")?, 0)
                .single()
                .context("create_time out of range")?;
            let contact = contacts.get(username);
            let display = contact
                .map(|c| {
                    if !c.remark.is_empty() {
                        c.remark.as_str()
                    } else {
                        c.nick_name.as_str()
                    }
                })
                .filter(|s| !s.is_empty())
                .unwrap_or(username);
            failure_stage = BatchFailureStage::Directory;
            let mut check = || {
                ensure!(!cancelled(), "Audio batch cancelled");
                source.verify()?;
                if let Some(pin) = &contact_pin {
                    pin.verify()?;
                }
                validate_output(options, &output)
            };
            let directory = contact_directory_checked(
                &output, username, display, contact, &protected, &mut check,
            )?;
            let ownership =
                crate::attachment::local_files::Pin::open(&directory.join(".info"), false)?;
            let voice = directory.join("voice");
            checked_mkdir(&voice)?;
            let target = voice.join(format!("{}_{local_id}.mp3", date.format("%Y%m%d_%H%M%S")));
            reject_reparse(&target)?;
            if target.exists() {
                ensure!(target.is_file(), "existing MP3 path is not a file");
                crate::infrastructure::publication::ExportTarget::capture_paths(
                    &target, &protected,
                )?;
                return Ok(BatchItemOutcome::Existing);
            }
            failure_stage = BatchFailureStage::Material;
            let data = material()?;
            let mut input = tempfile::NamedTempFile::new_in(&voice)?;
            input.write_all(&data)?;
            input.flush()?;
            let input = input.into_temp_path();
            let staged = tempfile::NamedTempFile::new_in(&voice)?.into_temp_path();
            failure_stage = BatchFailureStage::Conversion;
            crate::infrastructure::audio::convert_controlled(
                &input,
                &staged,
                &options.ffmpeg,
                std::time::Instant::now() + std::time::Duration::from_secs(120),
                &mut cancelled,
            )?;
            failure_stage = BatchFailureStage::Publication;
            let mut check = || {
                ensure!(!cancelled(), "Audio batch cancelled");
                ownership.verify()?;
                source.verify()?;
                if let Some(pin) = &contact_pin {
                    pin.verify()?;
                }
                validate_output(options, &output)
            };
            if publish_mp3_checked(staged, &target, &protected, &mut check)? {
                Ok(BatchItemOutcome::Converted)
            } else {
                Ok(BatchItemOutcome::Existing)
            }
        })();
        // Cancellation terminates the batch; it is not another bad input record.
        ensure!(!cancelled(), "Audio batch cancelled");
        match result {
            Ok(outcome) => report.progress.record(outcome),
            Err(_) => {
                report.progress.record(BatchItemOutcome::Failed);
                report.failures.push(BatchFailure {
                    chat_name_id: entry.chat_name_id,
                    local_id: entry.local_id,
                    error: failure_message(failure_stage).into(),
                });
            }
        }
        Ok(())
    })?;
    Ok(report)
}

#[cfg(test)]
fn publish_mp3(staged: tempfile::TempPath, target: &Path) -> Result<bool> {
    publish_mp3_checked(staged, target, &[], &mut || Ok(()))
}

fn publish_mp3_checked(
    staged: tempfile::TempPath,
    target: &Path,
    protected: &[PathBuf],
    check: &mut impl FnMut() -> Result<()>,
) -> Result<bool> {
    check()?;
    let mut protected = protected.to_vec();
    protected.push(staged.to_path_buf());
    let result = crate::infrastructure::publication::ExportTarget::new_file(target, &protected)
        .and_then(|target| {
            crate::infrastructure::audio::publish_encoded(&staged, target, &mut *check)
        });
    match result {
        Ok(()) => Ok(true),
        Err(error) => {
            check()?;
            reject_reparse(target)?;
            if target.is_file() {
                crate::infrastructure::publication::ExportTarget::capture_paths(
                    target, &protected,
                )?;
                // Preserve the legacy late-writer skip, never replace its file.
                Ok(false)
            } else {
                Err(error)
            }
        }
    }
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

#[cfg(test)]
fn contact_directory(
    root: &Path,
    username: &str,
    display: &str,
    contact: Option<&Contact>,
) -> Result<PathBuf> {
    contact_directory_checked(root, username, display, contact, &[], &mut || Ok(()))
}

fn contact_directory_checked(
    root: &Path,
    username: &str,
    display: &str,
    contact: Option<&Contact>,
    protected: &[PathBuf],
    check: &mut impl FnMut() -> Result<()>,
) -> Result<PathBuf> {
    check()?;
    let base = safe_dirname(display);
    let digest = format!("{:x}", md5::compute(username.as_bytes()));
    for name in [base.clone(), format!("{base}~{}", &digest[..12])] {
        let directory = root.join(name);
        checked_mkdir(&directory)?;
        let info = directory.join(".info");
        reject_reparse(&info)?;
        if info.exists() {
            crate::infrastructure::publication::ExportTarget::capture_paths(&info, protected)?;
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
        check()?;
        let result = crate::infrastructure::publication::ExportTarget::new_file(&info, protected)
            .and_then(|target| target.write_bytes_checked(contents.as_bytes(), &mut *check));
        match result {
            Ok(()) => return Ok(directory),
            Err(error) => {
                check()?;
                reject_reparse(&info)?;
                if info.is_file() {
                    if fs::read_to_string(&info)?.lines().next()
                        == Some(format!("username:  {username}").as_str())
                    {
                        return Ok(directory);
                    }
                } else {
                    return Err(error);
                }
            }
        }
    }
    anyhow::bail!("contact output directory belongs to another username")
}

#[cfg(test)]
#[path = "voice_batch_export/tests.rs"]
mod tests;
