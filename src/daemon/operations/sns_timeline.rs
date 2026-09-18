//! 选中配置的 SNS 生产宿主；不发现其他账号，不启动旧 Python。
use crate::infrastructure::output_tree::ExistingPolicy;
use crate::{
    adapters::wechat::moments::cache::{build_cache_index, CacheKeys, CacheLimits, CacheRoots},
    application::moments::{self as sns, export_database_with_publication, TimelinePublication},
    runtime::RuntimeContext,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub use crate::service::operation_requests::sns_timeline::Args;

fn options(
    args: &Args,
    contacts_env: Option<&str>,
    download_env: Option<&str>,
) -> Result<(sns::ExportOptions, bool)> {
    ensure!(
        !(args.download_media && args.no_remote),
        "--download-media 与 --no-remote 冲突"
    );
    let raw = args
        .contacts
        .as_deref()
        .or(contacts_env)
        .unwrap_or_default();
    Ok((
        sns::ExportOptions {
            contacts: raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            ..Default::default()
        },
        !args.no_remote && (args.download_media || download_env.is_some_and(|s| s.trim() == "1")),
    ))
}

fn paths(runtime: &RuntimeContext, raw: &Value, args: &Args) -> Result<(PathBuf, CacheRoots)> {
    let base = runtime.config_path.parent().context("选中配置缺少父目录")?;
    let db = &runtime.config.db_dir;
    let account = crate::adapters::wechat::moments::cache::account_root(db)?;
    let name = account.file_name().context("选中账号缺少目录名")?;
    // 旧 config.py 无条件覆盖这三个派生项；原生基准为选中 config 旁，而非 vendor。
    let output = std::path::absolute(
        args.output_dir
            .clone()
            .unwrap_or_else(|| base.join("wechat_files").join(name)),
    )?;
    // 仅接受配置声明的旧账号目录，不复刻 Documents 下跨账号的前缀模糊发现。
    let legacy = match raw.get("wechat_files_dir") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => {
            let path = PathBuf::from(s);
            Some(if path.is_absolute() {
                path
            } else {
                base.join(path)
            })
        }
        Some(_) => anyhow::bail!("wechat_files_dir 必须是路径字符串"),
    };
    Ok((output, CacheRoots::for_account(account, legacy.as_deref())))
}

#[cfg(test)]
fn image_keys(raw: &Value) -> Result<CacheKeys> {
    use crate::application::image_publication::{parse_aes, parse_xor};
    let aes = match raw.get("image_aes_key") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => {
            Some(parse_aes(s).map_err(|_| anyhow::anyhow!("image_aes_key 格式无效"))?)
        }
        Some(_) => anyhow::bail!("image_aes_key 必须是字符串"),
    };
    let xor = match raw.get("image_xor_key") {
        None => 0x88,
        Some(value) => parse_xor(
            &value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        )
        .map_err(|_| anyhow::anyhow!("image_xor_key 必须为 0 至 255 的十进制或十六进制字节"))?,
    };
    Ok(CacheKeys {
        image_aes_key: aes,
        image_xor_key: xor,
    })
}

fn existing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => anyhow::bail!("无法检查 SNS 输入路径"),
    }
}

fn publication(
    runtime: &RuntimeContext,
    roots: &CacheRoots,
    output: &Path,
    adopt: bool,
) -> Result<TimelinePublication> {
    let mut inputs = Vec::new();
    for source in [
        Some(&runtime.config_path),
        Some(&runtime.config.keys_file),
        Some(&runtime.config.db_dir),
        Some(&runtime.config.decrypted_dir),
        Some(&runtime.directory),
        roots.xwechat.as_ref(),
        roots.file_storage_sns.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        // 缺失路径也先做分离检查；现存输入交给 core 固定身份并检查硬链接。
        crate::infrastructure::publication::separate(source, output)
            .map_err(|_| anyhow::anyhow!("SNS 输出不能覆盖配置、密钥、数据库或缓存"))?;
        if existing(source)? {
            inputs.push(source.clone());
        }
    }
    Ok(TimelinePublication {
        source_kind: "account".into(),
        source_id: runtime.id.clone(),
        flat_cache: true,
        policy: if adopt {
            ExistingPolicy::Adopt
        } else {
            ExistingPolicy::Update
        },
        inputs,
    })
}

pub fn cmd(args: Args) -> Result<()> {
    let runtime = RuntimeContext::load().map_err(|_| anyhow::anyhow!("无法加载选中账号配置"))?;
    // 扩展字段只读同一文件，不再次调用配置发现或 vendor 的 loader。
    let bytes = zeroize::Zeroizing::new(
        fs::read(&runtime.config_path).map_err(|_| anyhow::anyhow!("无法读取选中账号配置"))?,
    );
    let raw: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("选中账号配置 JSON 无效"))?;
    let contacts_env = std::env::var("WECHAT_EXPORT_CONTACTS").ok();
    let download_env = std::env::var("WECHAT_SNS_DOWNLOAD_MEDIA").ok();
    let source_exists = existing(
        &runtime
            .config
            .decrypted_dir
            .join(crate::adapters::wechat::moments::source_key()),
    )?;
    let material = source_exists
        .then(|| crate::service::worker_keys::image_material(&runtime))
        .transpose()?
        .flatten();
    if source_exists {
        crate::service::worker_keys::verify_image_revision(&runtime)?;
    }
    export_for(
        &runtime,
        &raw,
        args,
        contacts_env.as_deref(),
        download_env.as_deref(),
        material.as_ref(),
    )?;
    if source_exists {
        crate::service::worker_keys::verify_image_revision(&runtime)?;
    }
    Ok(())
}

fn export_for(
    runtime: &RuntimeContext,
    raw: &Value,
    args: Args,
    contacts_env: Option<&str>,
    download_env: Option<&str>,
    material: Option<&crate::service::worker_keys::ImageMaterial>,
) -> Result<()> {
    let (options, remote) = options(&args, contacts_env, download_env)?;
    let sns_db = runtime
        .config
        .decrypted_dir
        .join(crate::adapters::wechat::moments::source_key());
    let contact_db = runtime.config.decrypted_dir.join("contact/contact.db");
    let missing = !existing(&sns_db)?;
    let (output, mut roots) = paths(runtime, raw, &args)?;
    let publication = publication(runtime, &roots, &output, args.adopt_existing)?;
    let keys = if missing {
        CacheKeys::default()
    } else {
        CacheKeys {
            image_aes_key: material.map(|material| material.aes),
            image_xor_key: material.map_or(0x88, |material| material.xor),
        }
    };
    // 旧脚本跳过不存在的缓存；存在但不可读或不安全的根仍由 core 报错。
    for root in [&mut roots.xwechat, &mut roots.file_storage_sns] {
        if root
            .as_ref()
            .map(|p| existing(p))
            .transpose()?
            .is_some_and(|present| !present)
        {
            *root = None;
        }
    }
    let cache = if !missing && (roots.xwechat.is_some() || roots.file_storage_sns.is_some()) {
        Some(
            build_cache_index(&roots, &keys, CacheLimits::default())
                .map_err(|_| anyhow::anyhow!("SNS 缓存索引失败；请检查选中账号缓存路径"))?,
        )
    } else {
        None
    };
    let config_pin = cache
        .as_ref()
        .map(|_| crate::service::config_pin::ConfigPin::new(runtime))
        .transpose()?;
    let verify = || -> Result<()> {
        config_pin
            .as_ref()
            .context("SNS 图片材料配置未固定")?
            .verify(runtime)?;
        crate::service::worker_keys::verify_image_revision(runtime)
    };
    let recovery = cache.as_ref().map(|index| sns::CacheRecovery {
        index,
        keys: &keys,
        verify: &verify,
    });
    let download = remote.then(sns::DownloadOptions::default);
    let mut report = if missing {
        // Keep report fields, but an absent source is not a successful empty timeline.
        Default::default()
    } else {
        export_database_with_publication(
            &sns_db,
            existing(&contact_db)?.then_some(contact_db.as_path()),
            &output,
            &options,
            recovery.as_ref(),
            download.as_ref(),
            &publication,
        )
        .context(
            "SNS 导出失败；请检查数据库、输出权限及来源绑定，未绑定旧目录须显式 --adopt-existing",
        )?
    };
    if missing {
        report
            .warnings
            .push("朋友圈数据库不可用，请先解密选中账号数据库；未改动输出目录".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "engine": "rust", "contacts": report.contacts, "posts": report.posts,
            "status": if missing { "unavailable" } else if report.media_failed > 0 || report.media_download_failed > 0 || report.invalid > 0 { "partial" } else { "exported" },
            "exit_code": if missing || report.media_failed > 0 || report.media_download_failed > 0 || report.invalid > 0 { 1 } else { 0 },
            "coverage": "local_cache_only",
            "files": report.files, "rows_seen": report.rows_seen, "filtered": report.filtered,
            "invalid": report.invalid, "warnings": report.warnings,
            "legacy_unverified": report.legacy_unverified,
            "cache_scanned": cache.as_ref().map(|i| i.scanned),
            "cache_images": cache.as_ref().map(|i| i.images().len()),
            "cache_videos": cache.as_ref().map(|i| i.videos().len()),
            "media_recovered": report.media_recovered, "media_missing": report.media_missing,
            "media_failed": report.media_failed, "media_downloaded": report.media_downloaded,
            "media_download_failed": report.media_download_failed,
        }))?
    );
    if missing {
        return Err(crate::business::moments::SourceError::Unavailable.into());
    }
    ensure!(
        report.media_failed == 0,
        "{} 个媒体恢复失败；详见 _media_recovery.json",
        report.media_failed
    );
    ensure!(
        report.media_download_failed == 0,
        "{} 个媒体下载失败；详见 _media_recovery.json",
        report.media_download_failed
    );
    ensure!(
        report.invalid == 0,
        "{} 条 SNS 记录无法解析；详见 warnings",
        report.invalid
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: crate::cli::operation_args::sns_timeline::Args,
    }

    fn runtime(root: &Path) -> RuntimeContext {
        let runtime = RuntimeContext {
            config: crate::config::Config {
                key_store: Some(root.join("keys.dpapi")),
                db_dir: root.join("accounts/selected/db_storage"),
                keys_file: root.join("all_keys.json"),
                decrypted_dir: root.join("decrypted"),
                wechat_process: String::new(),
            },
            config_path: root.join("config.json"),
            root: root.to_owned(),
            id: "selected-runtime-id".into(),
            directory: root.join("runtime/selected"),
        };
        fs::create_dir_all(&runtime.config.db_dir).unwrap();
        crate::key_store::Store::for_runtime(&runtime)
            .unwrap()
            .update(
                Some(0),
                &[crate::key_store::Update::Image(
                    b"syntheticAESkey1",
                    0x88,
                    crate::key_store::Verification::Verified,
                )],
            )
            .unwrap();
        runtime
    }

    #[test]
    fn clap_contract_and_remote_precedence() {
        assert!(Cli::try_parse_from(["sns", "--download-media", "--no-remote"]).is_err());
        let args: Args = Cli::try_parse_from([
            "sns",
            "--contacts",
            "a,b",
            "--output-dir",
            "out",
            "--adopt-existing",
        ])
        .unwrap()
        .args
        .into();
        assert_eq!(args.contacts.as_deref(), Some("a,b"));
        assert!(args.adopt_existing);
        for env in [
            None,
            Some("0"),
            Some("true"),
            Some("01"),
            Some("1"),
            Some(" 1 "),
        ] {
            assert_eq!(
                options(&Args::default(), None, env).unwrap().1,
                env.is_some_and(|s| s.trim() == "1")
            );
            assert!(
                !options(
                    &Args {
                        no_remote: true,
                        ..Default::default()
                    },
                    None,
                    env
                )
                .unwrap()
                .1
            );
            assert!(
                options(
                    &Args {
                        download_media: true,
                        ..Default::default()
                    },
                    None,
                    env
                )
                .unwrap()
                .1
            );
        }
        assert!(options(
            &Args {
                download_media: true,
                no_remote: true,
                ..Default::default()
            },
            None,
            None
        )
        .is_err());
    }

    #[test]
    fn contacts_cli_overrides_env_including_explicit_empty() {
        let (opts, _) = options(&Args::default(), Some(" a,b,a, "), None).unwrap();
        assert_eq!(opts.contacts.into_iter().collect::<Vec<_>>(), ["a", "b"]);
        let args = Args {
            contacts: Some(String::new()),
            ..Default::default()
        };
        assert!(options(&args, Some("other"), None)
            .unwrap()
            .0
            .contacts
            .is_empty());
    }

    #[test]
    fn legacy_derived_fields_use_selected_config_not_vendor_or_other_account() {
        let temp = tempfile::tempdir().unwrap();
        let rt = runtime(temp.path());
        let raw = json!({"output_base_dir":"ignored", "xwechat_cache_dir":"other", "sns_cache_dir":"other", "wechat_files_dir":"legacy-selected"});
        let (output, roots) = paths(&rt, &raw, &Args::default()).unwrap();
        assert_eq!(output, temp.path().join("wechat_files/selected"));
        assert_eq!(
            roots.xwechat.unwrap(),
            temp.path().join("accounts/selected/cache")
        );
        assert_eq!(
            roots.file_storage_sns.unwrap(),
            temp.path().join("legacy-selected/FileStorage/Sns/Cache")
        );
        assert!(paths(&rt, &json!({}), &Args::default())
            .unwrap()
            .1
            .file_storage_sns
            .is_none());
        let explicit = temp.path().join("chosen-output");
        assert_eq!(
            paths(
                &rt,
                &raw,
                &Args {
                    output_dir: Some(explicit.clone()),
                    ..Default::default()
                }
            )
            .unwrap()
            .0,
            explicit
        );
    }

    #[test]
    fn keys_reuse_ascii_aes_and_numeric_or_hex_xor_without_echo() {
        let keys = image_keys(
            &json!({"image_aes_key":"0123456789abcdefghijklmnopqrstuv", "image_xor_key":"0x88"}),
        )
        .unwrap();
        assert_eq!(keys.image_aes_key, Some(*b"0123456789abcdef"));
        assert_eq!(keys.image_xor_key, 136);
        assert_eq!(
            image_keys(&json!({"image_xor_key":7}))
                .unwrap()
                .image_xor_key,
            7
        );
        assert_eq!(image_keys(&json!({})).unwrap().image_xor_key, 0x88);
        for raw in [
            json!({"image_aes_key":"private"}),
            json!({"image_xor_key":"private"}),
            json!({"image_xor_key":256}),
        ] {
            let error = image_keys(&raw).err().unwrap();
            assert!(!format!("{error:#}").contains("private"));
        }
    }

    #[test]
    fn publication_binds_runtime_and_protects_existing_and_future_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let rt = runtime(temp.path());
        fs::write(&rt.config_path, b"{}").unwrap();
        fs::write(&rt.config.keys_file, b"synthetic").unwrap();
        let (out, roots) = paths(&rt, &json!({}), &Args::default()).unwrap();
        let policy = publication(&rt, &roots, &out, false).unwrap();
        assert_eq!(policy.source_kind, "account");
        assert_eq!(policy.source_id, rt.id);
        assert_eq!(policy.policy, ExistingPolicy::Update);
        assert!(policy.inputs.contains(&rt.config_path));
        assert!(policy.inputs.contains(&rt.config.keys_file));
        assert!(!policy.inputs.contains(&rt.root));
        assert!(!policy
            .inputs
            .contains(&rt.config_path.parent().unwrap().to_path_buf()));
        assert_eq!(
            publication(&rt, &roots, &out, true).unwrap().policy,
            ExistingPolicy::Adopt
        );
        for output in [
            &rt.config_path,
            &rt.config.keys_file,
            &rt.config.db_dir,
            &rt.config.decrypted_dir,
            &rt.directory,
            roots.xwechat.as_ref().unwrap(),
        ] {
            assert!(publication(&rt, &roots, output, false).is_err());
        }
        assert!(!out.exists());
    }

    #[test]
    fn missing_sns_db_is_unavailable_without_creating_output() {
        let temp = tempfile::tempdir().unwrap();
        let rt = runtime(temp.path());
        let error = export_for(&rt, &json!({}), Args::default(), None, None, None).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<crate::business::moments::SourceError>(),
            Some(crate::business::moments::SourceError::Unavailable)
        ));
        assert!(!temp.path().join("wechat_files").exists());
        assert!(!rt.config.decrypted_dir.exists());
    }

    #[test]
    fn empty_sns_and_missing_contacts_preserve_old_output() {
        let temp = tempfile::tempdir().unwrap();
        let rt = runtime(temp.path());
        fs::create_dir_all(rt.config.decrypted_dir.join("sns")).unwrap();
        let conn = rusqlite::Connection::open(rt.config.decrypted_dir.join("sns/sns.db")).unwrap();
        conn.execute_batch("CREATE TABLE SnsTimeLine (tid INTEGER, user_name TEXT, content TEXT);")
            .unwrap();
        drop(conn);
        let out = temp.path().join("old");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("sentinel"), b"unchanged").unwrap();
        export_for(
            &rt,
            &json!({}),
            Args {
                output_dir: Some(out.clone()),
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(fs::read(out.join("sentinel")).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
        assert!(!rt.config.decrypted_dir.join("contact").exists());
    }

    #[test]
    fn filtered_empty_preserves_old_tree_but_invalid_selected_row_fails() {
        let temp = tempfile::tempdir().unwrap();
        let rt = runtime(temp.path());
        fs::create_dir_all(rt.config.decrypted_dir.join("sns")).unwrap();
        let conn = rusqlite::Connection::open(rt.config.decrypted_dir.join("sns/sns.db")).unwrap();
        conn.execute_batch("CREATE TABLE SnsTimeLine (tid INTEGER, user_name TEXT, content TEXT); INSERT INTO SnsTimeLine VALUES (1, 'selected', '<invalid');").unwrap();
        drop(conn);
        let out = temp.path().join("old");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("sentinel"), b"unchanged").unwrap();
        export_for(
            &rt,
            &json!({}),
            Args {
                contacts: Some("other".into()),
                output_dir: Some(out.clone()),
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .unwrap();
        let error = export_for(
            &rt,
            &json!({}),
            Args {
                contacts: Some("selected".into()),
                output_dir: Some(out.clone()),
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("1 条 SNS 记录无法解析"));
        assert_eq!(fs::read(out.join("sentinel")).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
    }
}
