//! Explicit local-only legacy import. Ordinary reads never migrate or delete files.
use crate::{
    key_store::{Store, Update, Verification},
    runtime::RuntimeContext,
    toolkit::setup::{ConfigDocument, Snapshot},
};
use anyhow::{ensure, Context, Result};
use std::collections::HashMap;
use zeroize::{Zeroize, Zeroizing};

#[derive(Default)]
struct DatabaseMaterial(HashMap<String, String>);
impl std::ops::Deref for DatabaseMaterial {
    type Target = HashMap<String, String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for DatabaseMaterial {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for DatabaseMaterial {
    fn drop(&mut self) {
        self.0.values_mut().for_each(Zeroize::zeroize);
    }
}

struct MigrationDocument(ConfigDocument);
impl std::ops::Deref for MigrationDocument {
    type Target = ConfigDocument;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for MigrationDocument {
    fn drop(&mut self) {
        for field in ["image_aes_key", "image_xor_key"] {
            if let Some(serde_json::Value::String(value)) = self.0.value.get_mut(field) {
                value.zeroize();
            }
        }
    }
}

pub use crate::service::operation_requests::key_migration::Args;

pub fn cmd(args: Args) -> Result<()> {
    let runtime = RuntimeContext::load()?;
    let report = migrate(&runtime, &args)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

pub(crate) fn migrate(runtime: &RuntimeContext, args: &Args) -> Result<serde_json::Value> {
    let document = MigrationDocument(ConfigDocument::load(&runtime.config_path)?);
    crate::key_store::legacy_json::parse(
        document
            .snapshot
            .bytes()
            .context("Migration requires an existing account configuration")?,
    )?;
    let _lock = document.lock()?;
    document.ensure_account(&runtime.config.db_dir)?;
    let current = crate::config::load_config_at(&runtime.config_path)?;
    let current_runtime = RuntimeContext::from_config(
        runtime.config_path.clone(),
        current.clone(),
        runtime.root.clone(),
    )?;
    ensure!(
        runtime.same_account(&current_runtime)?,
        "Migration account configuration changed"
    );
    let legacy_db = Snapshot::capture(&current.keys_file)?;
    let legacy_account = Snapshot::capture(&document.base().join("account_key.dpapi"))?;
    let mut verified = DatabaseMaterial::default();
    let mut unverified = DatabaseMaterial::default();
    if legacy_db.existed() {
        let keys = DatabaseMaterial(
            super::toolkit_run_prepare::decode_legacy(runtime, legacy_db.bytes().unwrap())
                .map_err(|_| {
                    anyhow::anyhow!("Legacy database keys are invalid or belong to another account")
                })?,
        );
        let outcome = (|| -> Result<()> {
            let metadata = crate::key_store::legacy_json::parse(legacy_db.bytes().unwrap())
                .map_err(|_| anyhow::anyhow!("Invalid legacy database key document"))?;
            let explicitly_bound = metadata
                .get("_db_dir")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.is_empty());
            for (name, key) in keys.iter() {
                if runtime
                    .config
                    .db_dir
                    .join(name.replace('\\', "/"))
                    .try_exists()?
                {
                    super::toolkit_run_prepare::validate_keys(
                        runtime,
                        &DatabaseMaterial(HashMap::from([(name.clone(), key.clone())])),
                    )
                    .map_err(|_| anyhow::anyhow!("Legacy database key failed local validation"))?;
                    verified.insert(name.clone(), key.clone());
                } else {
                    ensure!(args.allow_unverified && explicitly_bound,
                        "Missing database evidence: explicit account binding and --allow-unverified are required");
                    unverified.insert(name.clone(), key.clone());
                }
            }
            Ok(())
        })();
        outcome?;
    }
    let account = if legacy_account.existed() {
        Some(
            crate::scanner::load_legacy_account(&runtime.config.db_dir, &legacy_account.path)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Legacy account key is invalid, inaccessible or belongs to another account"
                    )
                })?,
        )
    } else {
        None
    };
    let mut account_verification = Verification::Verified;
    if let Some(key) = account.as_ref() {
        let pages = crate::scanner::collect_db_salts(&runtime.config.db_dir);
        if pages.is_empty() {
            ensure!(
                args.allow_unverified,
                "No account-key verification evidence; --allow-unverified is required"
            );
            account_verification = Verification::Unverified;
        } else {
            let mut entries = crate::scanner::verify_account_material(&runtime.config.db_dir, key)
                .map_err(|_| anyhow::anyhow!("Legacy account key failed local validation"))?;
            for entry in &mut entries {
                entry.enc_key.zeroize();
            }
        }
    }
    let legacy_xor = match document.value.get("image_xor_key") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(value)) => Some(
            value
                .as_u64()
                .filter(|v| *v <= 255)
                .context("Invalid legacy XOR material")? as u8,
        ),
        Some(serde_json::Value::String(value)) => Some(
            crate::toolkit::parse_image_xor(value)
                .map_err(|_| anyhow::anyhow!("Invalid legacy XOR material"))?,
        ),
        _ => anyhow::bail!("Invalid legacy XOR material"),
    };
    let image = match document.value.get("image_aes_key") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) if value.is_empty() => None,
        Some(value) => {
            let aes = Zeroizing::new(
                crate::toolkit::parse_image_aes(
                    value.as_str().context("Invalid legacy image material")?,
                )
                .map_err(|_| anyhow::anyhow!("Invalid legacy image material"))?,
            );
            let xor = legacy_xor.unwrap_or(0x88);
            use crate::attachment::image_key::windows::ExistingKeyEvidence;
            let evidence =
                crate::attachment::image_key::windows::validate_existing_evidence_for_db_dir(
                    &runtime.config.db_dir,
                    &aes,
                    std::time::Duration::from_secs(30),
                    64 * 1024 * 1024,
                )?;
            let verification = match evidence {
                ExistingKeyEvidence::Verified => Verification::Verified,
                ExistingKeyEvidence::NoEvidence => {
                    ensure!(
                        args.allow_unverified,
                        "No image verification evidence; --allow-unverified is required"
                    );
                    Verification::Unverified
                }
                ExistingKeyEvidence::Contradicted => anyhow::bail!(
                    "Legacy image material contradicts local evidence; migration refused"
                ),
            };
            Some((aes, xor, verification))
        }
    };
    let mut selected = current.clone();
    ensure!((image.is_none() && legacy_xor.is_none()) || args.cleanup_legacy,
        "Migrating legacy image fields requires --cleanup-legacy to avoid rewriting plaintext material");
    selected
        .key_store
        .get_or_insert_with(|| document.base().join("keys.dpapi"));
    let mut selected_runtime = runtime.clone();
    selected_runtime.config = selected.clone();
    let store = Store::for_runtime(&selected_runtime)?;
    let mut updates = Vec::new();
    if !verified.is_empty() {
        updates.push(Update::Databases(&verified, Verification::Verified));
    }
    if !unverified.is_empty() {
        updates.push(Update::Databases(&unverified, Verification::Unverified));
    }
    if let Some(key) = account.as_ref() {
        updates.push(Update::Account(key, account_verification));
    }
    if let Some((aes, xor, status)) = image.as_ref() {
        updates.push(Update::Image(aes, *xor, *status));
    } else if let Some(xor) = legacy_xor {
        ensure!(
            args.allow_unverified,
            "XOR-only material requires --allow-unverified without local validation evidence"
        );
        updates.push(Update::ImageXor(xor, Verification::Unverified));
    }
    ensure!(
        !updates.is_empty() || selected.key_store.as_ref().is_some_and(|p| p.is_file()),
        "No legacy key material to migrate"
    );
    document.snapshot.verify()?;
    legacy_db.verify()?;
    legacy_account.verify()?;
    let imported = if updates.is_empty() {
        store.load()
    } else {
        store.import(&updates)
    };
    verified.values_mut().for_each(Zeroize::zeroize);
    unverified.values_mut().for_each(Zeroize::zeroize);
    let imported = imported?;
    let (verified_count, unverified_count) = imported.counts();
    let mut value = document.value.clone();
    if current.key_store.is_none() {
        value["key_store"] = serde_json::json!(store.path());
    }
    if args.cleanup_legacy {
        if let Some(serde_json::Value::String(mut secret)) =
            value.as_object_mut().unwrap().remove("image_aes_key")
        {
            secret.zeroize();
        }
        value.as_object_mut().unwrap().remove("image_xor_key");
    }
    legacy_db.verify()?;
    legacy_account.verify()?;
    if value != document.value {
        document.snapshot.write_json(
            &value,
            &[
                runtime.config.db_dir.clone(),
                current.keys_file.clone(),
                store.path().to_owned(),
            ],
        )?;
    } else {
        document.snapshot.verify()?;
    }
    Ok(
        serde_json::json!({"account_id":runtime.id, "revision":imported.revision(),
        "verified_materials":verified_count, "unverified_materials":unverified_count,
        "legacy_image_fields_removed":args.cleanup_legacy, "legacy_database_file_retained":legacy_db.existed(),
        "legacy_file_retention_reason":"exclusivity_not_established",
        "legacy_account_file_retained":legacy_account.existed(), "keys_redacted":true}),
    )
}

#[cfg(test)]
#[path = "key_migration_tests.rs"]
mod tests;

#[cfg(test)]
mod image_validation_tests {
    use super::*;
    use std::fs;
    const AES: &[u8; 16] = b"syntheticAESkey1";

    fn fixture() -> Result<(tempfile::TempDir, RuntimeContext, Vec<u8>)> {
        let root = tempfile::tempdir()?;
        let config = crate::config::Config {
            key_store: None,
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("legacy-keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        fs::create_dir_all(&config.db_dir)?;
        let path = root.path().join("config.json");
        let mut value = serde_json::to_value(&config)?;
        value.as_object_mut().unwrap().remove("key_store");
        value["image_aes_key"] = serde_json::json!(std::str::from_utf8(AES)?);
        value["image_xor_key"] = serde_json::json!(0xa2);
        let original = serde_json::to_vec(&value)?;
        fs::write(&path, &original)?;
        let runtime = RuntimeContext::from_config(
            path.clone(),
            crate::config::load_config_at(&path)?,
            root.path().join("runtime"),
        )?;
        Ok((root, runtime, original))
    }

    #[test]
    fn image_migration_fixture_null_reference_fails_before_image_validation() -> Result<()> {
        let (_root, runtime, original) = fixture()?;
        let mut value: serde_json::Value = serde_json::from_slice(&original)?;
        value["key_store"] = serde_json::Value::Null;
        fs::write(&runtime.config_path, serde_json::to_vec(&value)?)?;
        let error = migrate(
            &runtime,
            &Args {
                allow_unverified: true,
                cleanup_legacy: false,
            },
        )
        .unwrap_err();
        eprintln!("Synthetic null-reference configuration error: {error:#}");
        assert!(error.to_string().contains("key_store"), "{error:#}");
        Ok(())
    }

    #[test]
    fn image_migration_no_evidence_requires_explicit_flag() -> Result<()> {
        let (root, runtime, original) = fixture()?;
        let refused = migrate(
            &runtime,
            &Args {
                allow_unverified: false,
                cleanup_legacy: true,
            },
        )
        .unwrap_err();
        assert!(
            refused.to_string().contains("--allow-unverified"),
            "{refused:#}"
        );
        assert_eq!(fs::read(&runtime.config_path)?, original);
        assert!(!root.path().join("keys.dpapi").exists());
        let result = migrate(
            &runtime,
            &Args {
                allow_unverified: true,
                cleanup_legacy: true,
            },
        )?;
        assert_eq!(result["unverified_materials"], 1);
        assert_eq!(result["verified_materials"], 0);
        let config = crate::config::load_config_at(&runtime.config_path)?;
        let store = Store::for_config(&config)?;
        assert_eq!(store.load()?.image_material(), (Some(*AES), 0xa2));
        let cleaned: serde_json::Value = serde_json::from_slice(&fs::read(&runtime.config_path)?)?;
        assert!(cleaned.get("image_aes_key").is_none());
        assert!(cleaned.get("image_xor_key").is_none());
        assert!(!fs::read(store.path())?
            .windows(AES.len())
            .any(|bytes| bytes == AES));
        Ok(())
    }

    #[test]
    fn image_migration_mismatch_rejects_even_with_allow_unverified() -> Result<()> {
        use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
        let (root, runtime, original) = fixture()?;
        let attach = runtime.config.db_dir.parent().unwrap().join("msg/attach");
        fs::create_dir_all(&attach)?;
        let mut block = [0u8; 16];
        block[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        aes::Aes128::new(b"differentAESkey1".into())
            .encrypt_block(GenericArray::from_mut_slice(&mut block));
        let mut sample = vec![0u8; 15];
        sample[..6].copy_from_slice(&crate::attachment::decoder::V2_MAGIC);
        sample.extend_from_slice(&block);
        fs::write(attach.join("synthetic_t.dat"), sample)?;
        for allow_unverified in [false, true] {
            let error = migrate(
                &runtime,
                &Args {
                    allow_unverified,
                    cleanup_legacy: true,
                },
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("contradicts local evidence"),
                "{error:#}"
            );
            assert!(!format!("{error:#}").contains(std::str::from_utf8(AES)?));
            assert_eq!(fs::read(&runtime.config_path)?, original);
            assert!(!root.path().join("keys.dpapi").exists());
        }
        Ok(())
    }
}
