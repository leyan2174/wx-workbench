//! 原生配置向导；默认 dry-run，非 TTY 不读取 stdin，不接触扫描器。
use crate::infrastructure::configuration::{self, ConfigDocument};
use crate::infrastructure::publication as path_guard;
use crate::service::operation_requests::setup_native::argument_fingerprint;
use crate::service::operations::SetupReview;
use anyhow::{ensure, Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{self, BufRead, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied {
        config_path: PathBuf,
        created: bool,
    },
    Preview {
        config_path: PathBuf,
    },
    Cancelled {
        config_path: PathBuf,
    },
    Checked {
        config_path: PathBuf,
        config_exists: bool,
    },
}

#[derive(Debug)]
struct WizardCancelled;
impl std::fmt::Display for WizardCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("用户取消配置向导")
    }
}
impl std::error::Error for WizardCancelled {}

pub use crate::service::operation_requests::setup_native::Args;

fn ask(label: &str, default: &str) -> Result<String> {
    ensure!(
        io::stdin().is_terminal() && io::stderr().is_terminal(),
        "非 TTY 环境不能启动向导，请提供显式参数"
    );
    eprint!("{label} [{default}]（:cancel 取消）: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    let count = io::stdin().lock().take(4097).read_line(&mut answer)?;
    if count == 0 {
        return Err(WizardCancelled.into());
    }
    ensure!(answer.len() <= 4096, "向导输入超过长度限制");
    let answer = answer.trim();
    if answer == ":cancel" {
        return Err(WizardCancelled.into());
    }
    Ok(if answer.is_empty() {
        default.into()
    } else {
        answer.into()
    })
}

fn selected_db(base: &Path, path: &Path) -> Result<PathBuf> {
    let mut selected = path_guard::resolve(base, path)?;
    let _parent_guard = crate::attachment::local_files::HostOutputGuard::new(&selected)?;
    // 仅尝试用户指明目录的直属 db_storage，不枚举账号或按活跃度猜测。
    if !selected
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("db_storage"))
    {
        let child = selected.join("db_storage");
        if path_guard::exists(&child)? {
            let _child_guard = crate::attachment::local_files::HostOutputGuard::new(&child)?;
            selected = child;
        }
    }
    Ok(selected)
}

pub fn cmd(args: Args) -> Result<()> {
    run(args).map(|_| ())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Args) {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("account/db_storage");
        fs::create_dir_all(&db).unwrap();
        let args = Args {
            config_path: Some(root.path().join("settings/config.json")),
            db_dir: Some(db),
            ..Args::default()
        };
        (root, args)
    }

    #[test]
    fn default_preview_creates_no_configuration_directory() {
        let (root, args) = fixture();
        let config_path = args.config_path.clone().unwrap();
        assert!(!args.apply && !args.dry_run);
        assert_eq!(run(args).unwrap(), Outcome::Preview { config_path });
        assert!(!root.path().join("settings").exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn reviewed_first_run_commits_once_and_preserves_atomic_snapshot_check() {
        let (_root, mut args) = fixture();
        let path = config_path(&args).unwrap();
        let document = ConfigDocument::load(&path).unwrap();
        let review = review_for(&args, &document).unwrap();
        assert!(review.snapshot_sha256.is_none());
        preview(args.clone()).unwrap();
        assert!(!path.exists());
        args.apply = true;
        args.yes = true;
        apply_reviewed(args.clone(), &review).unwrap();
        let committed = fs::read(&path).unwrap();
        assert!(apply_reviewed(args, &review).is_err());
        assert_eq!(fs::read(path).unwrap(), committed);
    }

    #[test]
    fn reviewed_setup_rejects_changed_raw_config_even_if_json_is_equivalent() {
        let (root, mut args) = fixture();
        let path = config_path(&args).unwrap();
        fs::create_dir_all(root.path().join("settings")).unwrap();
        fs::write(&path, b"{}").unwrap();
        let document = ConfigDocument::load(&path).unwrap();
        let review = review_for(&args, &document).unwrap();
        fs::write(&path, b"{ }\n").unwrap();
        args.apply = true;
        args.yes = true;
        let error = apply_reviewed(args, &review).unwrap_err();
        assert!(error.to_string().contains("预览后发生变化"));
        assert_eq!(fs::read(path).unwrap(), b"{ }\n");
    }

    #[test]
    fn reviewed_setup_rejects_changed_arguments_without_creating_configuration() {
        let (_root, mut args) = fixture();
        let path = config_path(&args).unwrap();
        let document = ConfigDocument::load(&path).unwrap();
        let review = review_for(&args, &document).unwrap();
        args.db_dir = Some("changed-after-preview".into());
        args.apply = true;
        args.yes = true;
        assert!(apply_reviewed(args, &review).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn default_preview_preserves_existing_config_and_custom_keys() {
        let (root, args) = fixture();
        let config_path = args.config_path.clone().unwrap();
        fs::create_dir_all(root.path().join("settings/private")).unwrap();
        let keys = root.path().join("settings/private/selected.json");
        fs::write(&keys, b"synthetic existing keys").unwrap();
        let original = serde_json::to_vec(&json!({
            "db_dir": args.db_dir, "keys_file": "private/selected.json",
            "unknown": {"retain": [1, 2]}, "custom_setting": "retained-value"
        }))
        .unwrap();
        fs::write(&config_path, &original).unwrap();
        assert_eq!(
            run(args).unwrap(),
            Outcome::Preview {
                config_path: config_path.clone()
            }
        );
        assert_eq!(fs::read(&config_path).unwrap(), original);
        assert_eq!(fs::read(keys).unwrap(), b"synthetic existing keys");
        assert!(!root
            .path()
            .join("settings/.config.json.wx-setup.lock")
            .exists());
        assert!(!root.path().join("settings/all_keys.json").exists());
    }

    #[test]
    fn explicit_apply_yes_creates_then_updates_without_replacing_custom_keys() {
        let (root, mut args) = fixture();
        let config_path = args.config_path.clone().unwrap();
        args.apply = true;
        args.yes = true;
        assert_eq!(
            run(args).unwrap(),
            Outcome::Applied {
                config_path: config_path.clone(),
                created: true
            }
        );
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        value["keys_file"] = json!("private/selected.json");
        value["unknown"] = json!({"retain": [1, 2]});
        fs::create_dir(root.path().join("settings/private")).unwrap();
        let keys = root.path().join("settings/private/selected.json");
        fs::write(&keys, b"synthetic existing keys").unwrap();
        fs::write(&config_path, serde_json::to_vec(&value).unwrap()).unwrap();
        let result = run(Args {
            config_path: Some(config_path.clone()),
            apply: true,
            yes: true,
            ..Args::default()
        })
        .unwrap();
        assert_eq!(
            result,
            Outcome::Applied {
                config_path: config_path.clone(),
                created: false
            }
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
        assert_eq!(saved["keys_file"], value["keys_file"]);
        assert_eq!(saved["unknown"], value["unknown"]);
        assert_eq!(fs::read(keys).unwrap(), b"synthetic existing keys");
        assert!(!root.path().join("settings/all_keys.json").exists());
        assert!(!root.path().join("settings/account_key.dpapi").exists());
        assert!(!root.path().join("settings/decrypted").exists());
    }

    #[test]
    fn explicit_apply_preserves_malformed_config_without_echoing_its_contents() {
        let (root, mut args) = fixture();
        let config_path = args.config_path.clone().unwrap();
        fs::create_dir(root.path().join("settings")).unwrap();
        let original = b"{\"openai_api_key\":\"SYNTHETIC_SECRET_DO_NOT_ECHO\", broken";
        fs::write(&config_path, original).unwrap();
        args.apply = true;
        args.yes = true;
        let error = run(args).unwrap_err();
        assert!(!format!("{error:#}").contains("SYNTHETIC_SECRET_DO_NOT_ECHO"));
        assert_eq!(fs::read(config_path).unwrap(), original);
        assert_eq!(
            fs::read_dir(root.path().join("settings")).unwrap().count(),
            1
        );
    }
}

/// 首次启动器只在 Applied 后继续 GUI；Preview/Cancelled 都不会创建配置。
pub fn run(args: Args) -> Result<Outcome> {
    let config_path = match &args.config_path {
        Some(path) => std::path::absolute(path)?,
        None => std::path::absolute(crate::config::find_config_file()?)?,
    };
    let result = match run_at(args, config_path.clone()) {
        Err(error) if error.downcast_ref::<WizardCancelled>().is_some() => {
            Ok(Outcome::Cancelled { config_path })
        }
        result => result,
    };
    if let Ok(Outcome::Cancelled { config_path }) = &result {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"config_path":config_path,"applied":false,"cancelled":true})
            )?
        );
    }
    result
}

fn config_path(args: &Args) -> Result<PathBuf> {
    std::path::absolute(match &args.config_path {
        Some(path) => path.clone(),
        None => crate::config::find_config_file()?,
    })
    .map_err(Into::into)
}

fn review_for(args: &Args, document: &ConfigDocument) -> Result<SetupReview> {
    document.snapshot.verify()?;
    let snapshot_sha256 = if document.snapshot.existed() {
        use std::os::windows::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x00200000)
            .open(&document.snapshot.path)?;
        document.snapshot.verify()?;
        let mut hash = Sha256::new();
        let mut total = 0usize;
        let mut buffer = zeroize::Zeroizing::new([0u8; 8192]);
        loop {
            let count = file.read(buffer.as_mut())?;
            if count == 0 {
                break;
            }
            total += count;
            ensure!(total <= 16 * 1024 * 1024, "配置超过 16 MiB 限制");
            hash.update(&buffer[..count]);
        }
        document.snapshot.verify()?;
        Some(format!("{:x}", hash.finalize()))
    } else {
        None
    };
    Ok(SetupReview {
        config_path: document.snapshot.path.clone(),
        snapshot_sha256,
        arguments_sha256: argument_fingerprint(args, &document.snapshot.path)?,
    })
}

pub(super) fn preview(args: Args) -> Result<()> {
    let path = config_path(&args)?;
    run_at_review(args, path, None, true).map(|_| ())
}

pub(super) fn apply_reviewed(args: Args, review: &SetupReview) -> Result<()> {
    let path = config_path(&args)?;
    run_at_review(args, path, Some(review), false).map(|_| ())
}

fn run_at(args: Args, config_path: PathBuf) -> Result<Outcome> {
    run_at_review(args, config_path, None, false)
}

fn run_at_review(
    mut args: Args,
    config_path: PathBuf,
    expected: Option<&SetupReview>,
    capture: bool,
) -> Result<Outcome> {
    let document = match ConfigDocument::load(&config_path) {
        Ok(document) => document,
        Err(error) if args.check => {
            let mut report = configuration::environment(&config_path, &json!({}), false)?;
            report["config_exists"] = serde_json::Value::Null;
            report["config_status"] = json!("invalid_unreadable_or_unsafe");
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    let review = if capture || expected.is_some() {
        let review = review_for(&args, &document)?;
        if let Some(expected) = expected {
            ensure!(
                &review == expected,
                "配置或参数已在预览后发生变化；请重新预览并确认，未写入配置"
            );
        }
        Some(review)
    } else {
        None
    };
    if args.check {
        println!(
            "{}",
            serde_json::to_string_pretty(&configuration::environment(
                &config_path,
                &document.value,
                document.snapshot.existed()
            )?)?
        );
        return Ok(Outcome::Checked {
            config_path,
            config_exists: document.snapshot.existed(),
        });
    }
    ensure!(!(args.apply && args.dry_run), "--apply 与 --dry-run 冲突");
    let tty = io::stdin().is_terminal() && io::stderr().is_terminal();
    ensure!(!args.interactive || tty, "非 TTY 环境不能等待向导输入");
    ensure!(
        !args.apply || args.yes || tty,
        "非 TTY 写入需要 --apply --yes；缺省只预览"
    );
    let explicit = args.db_dir.is_some();
    let interactive = args.interactive || (tty && !explicit && !args.yes);
    let previous_db = document.configured_db()?;
    if interactive && args.db_dir.is_none() {
        let default = previous_db.as_deref().and_then(Path::to_str).unwrap_or("");
        args.db_dir = Some(PathBuf::from(ask("账号 db_storage 目录", default)?));
    }
    let db = match args.db_dir {
        Some(path) => selected_db(&std::env::current_dir()?, &path)?,
        None => {
            previous_db.context("尚未选择账号；请提供 --db-dir，非 TTY 不会自动选择或等待输入")?
        }
    };
    let account_guard = crate::attachment::local_files::HostOutputGuard::new(&db)?;
    let value = document.with_db(&db)?;
    let paths = document.validate_targets(&value)?;
    let changed: Vec<_> = value
        .as_object()
        .expect("配置为对象")
        .iter()
        .filter(|(key, v)| document.value.get(*key) != Some(*v))
        .map(|(key, _)| key.clone())
        .collect();
    // 只输出白名单摘要；未知字段及所有凭据值均不进入 stdout/stderr。
    let mut report = json!({"engine":"rust", "config_path":config_path, "db_dir":db,
        "keys_file":paths.keys_file,
        "changed_fields":changed, "applied":false,
        "preserved_original_fields":document.value.as_object().expect("配置为对象").len(),
        "environment":configuration::environment(&config_path, &value, document.snapshot.existed())?});
    if args.apply {
        if !args.yes {
            eprintln!(
                "配置目标: {}；账号: {}",
                config_path.display(),
                db.display()
            );
            if ask("确认写入请输入 APPLY", "取消")? != "APPLY" {
                return Ok(Outcome::Cancelled { config_path });
            }
        }
        let _lock = document.lock()?;
        account_guard.verify()?;
        let mut protected = paths.protected;
        protected.extend([paths.keys_file, paths.account_key_file]);
        document.snapshot.write_json(&value, &protected)?;
        report["applied"] = json!(true);
    }
    if capture {
        document.snapshot.verify()?;
        report["review"] = serde_json::to_value(review.expect("captured review"))?;
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(if args.apply {
        Outcome::Applied {
            config_path,
            created: !document.snapshot.existed(),
        }
    } else {
        Outcome::Preview { config_path }
    })
}
