//! Host-only bounded image material input; ordinary IPC contains only a sealed envelope.
use crate::{
    runtime::RuntimeContext,
    service::{config_pin::ConfigPin, worker_keys::ImageMaterial},
};
use anyhow::{ensure, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::io::Read;
use zeroize::{Zeroize, Zeroizing};

pub const MAX_INPUT_BYTES: usize = 4096;
const MAX_ENVELOPE_BYTES: usize = 8192;
const MAX_PROTECTED_BYTES: usize = 6144;
const PURPOSE: &str = "wx.image-material.import.v1";
fn invalid() -> anyhow::Error {
    anyhow::anyhow!("Invalid protected image import")
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub version: u32,
    pub runtime_id: String,
    pub config_sha256: String,
    pub revision: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[serde(deserialize_with = "bounded_envelope")]
    envelope: String,
}
impl std::fmt::Debug for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImageImport([REDACTED])")
    }
}
impl Drop for Args {
    fn drop(&mut self) {
        self.envelope.zeroize();
    }
}
fn bounded_envelope<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    struct Visitor;
    impl serde::de::Visitor<'_> for Visitor {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded protected envelope")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<String, E> {
            if value.is_empty() || value.len() > MAX_ENVELOPE_BYTES {
                return Err(E::custom("Invalid protected envelope size"));
            }
            Ok(value.to_owned())
        }
    }
    deserializer.deserialize_str(Visitor)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    pub no_save: bool,
    pub timeout: u64,
    pub max_mib: usize,
    pub sample_root: Option<std::path::PathBuf>,
}
impl ImportOptions {
    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=3600).contains(&self.timeout) && (1..=32768).contains(&self.max_mib),
            "Invalid image import budget"
        );
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Opened {
    version: u32,
    purpose: String,
    runtime_id: String,
    config_sha256: String,
    pub expected_revision: u64,
    pub options: ImportOptions,
    pub material: ImageMaterial,
    pub sample_root: std::path::PathBuf,
    pub sample_identity: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostInput {
    #[serde(deserialize_with = "aes_hex")]
    aes_key: Zeroizing<[u8; 16]>,
    xor_key: u8,
}
impl Drop for HostInput {
    fn drop(&mut self) {
        self.xor_key.zeroize();
    }
}
fn aes_hex<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Zeroizing<[u8; 16]>, D::Error> {
    struct Visitor;
    impl serde::de::Visitor<'_> for Visitor {
        type Value = Zeroizing<[u8; 16]>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("AES-128 hex material")
        }
        fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Self::Value, E> {
            if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(E::custom("Invalid image material"));
            }
            let mut key = Zeroizing::new([0; 16]);
            for (i, byte) in key.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
                    .map_err(|_| E::custom("Invalid image material"))?;
            }
            Ok(key)
        }
    }
    deserializer.deserialize_str(Visitor)
}

pub fn seal_stdin(
    runtime: &RuntimeContext,
    input: impl Read,
    options: ImportOptions,
) -> Result<Args> {
    options.validate()?;
    ensure!(
        !runtime.is_bootstrap(),
        "Image import requires a fixed account"
    );
    let pin = ConfigPin::new(runtime)?;
    let sample_root = std::path::absolute(options.sample_root.clone().unwrap_or_else(|| {
        crate::attachment::image_key::attach_root_for_db_dir(&runtime.config.db_dir)
    }))?;
    let (sample_guard, sample_identity) =
        crate::attachment::image_key::pin_image_root(&sample_root).map_err(|_| invalid())?;
    let mut bytes = Zeroizing::new(Vec::new());
    input
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_INPUT_BYTES,
        "Invalid image import input size"
    );
    let input: HostInput = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    crate::service::query_client::ensure_running_quiet(runtime)?;
    let metadata: Metadata = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            crate::service::client::wait_ready(runtime).await?;
            let value = crate::service::client::request(
                runtime,
                super::protocol::Call::ImageMaterialImportMetadata {},
            )
            .await?;
            serde_json::from_value(value).map_err(|_| invalid())
        })?;
    ensure!(
        metadata.version == 1
            && metadata.runtime_id == runtime.id
            && metadata.config_sha256 == pin.fingerprint()?,
        "Image import metadata binding mismatch"
    );
    let expected_revision = metadata.revision;
    let opened = Opened {
        version: 1,
        purpose: PURPOSE.into(),
        runtime_id: runtime.id.clone(),
        config_sha256: pin.fingerprint()?,
        expected_revision,
        options,
        material: ImageMaterial {
            aes: *input.aes_key,
            xor: input.xor_key,
        },
        sample_root,
        sample_identity,
    };
    let args = seal_opened(&opened)?;
    pin.verify(runtime)?;
    sample_guard.verify().map_err(|_| invalid())?;
    Ok(args)
}

fn seal_opened(opened: &Opened) -> Result<Args> {
    let plain = Zeroizing::new(serde_json::to_vec(opened).map_err(|_| invalid())?);
    ensure!(plain.len() <= MAX_INPUT_BYTES, "Invalid image import size");
    let protected = crate::key_store::dpapi::transform(&plain, false).map_err(|_| invalid())?;
    ensure!(
        protected.len() <= MAX_PROTECTED_BYTES,
        "Invalid protected image import size"
    );
    let args = Args {
        envelope: base64::engine::general_purpose::STANDARD.encode(&protected),
    };
    args.validate()?;
    Ok(args)
}

impl Args {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.envelope.is_empty() && self.envelope.len() <= MAX_ENVELOPE_BYTES,
            "Invalid protected image import size"
        );
        let decoded = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(&self.envelope)
                .map_err(|_| invalid())?,
        );
        ensure!(
            !decoded.is_empty() && decoded.len() <= MAX_PROTECTED_BYTES,
            "Invalid protected image import size"
        );
        Ok(())
    }
    pub(crate) fn open(&self, runtime: &RuntimeContext) -> Result<Opened> {
        self.validate()?;
        let protected = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(&self.envelope)
                .map_err(|_| invalid())?,
        );
        let plain = crate::key_store::dpapi::transform(&protected, true).map_err(|_| invalid())?;
        ensure!(plain.len() <= MAX_INPUT_BYTES, "Invalid image import size");
        let opened: Opened = serde_json::from_slice(&plain).map_err(|_| invalid())?;
        ensure!(
            opened.version == 1
                && opened.purpose == PURPOSE
                && opened.runtime_id == runtime.id
                && opened.config_sha256 == ConfigPin::new(runtime)?.fingerprint()?,
            "Protected image import binding mismatch"
        );
        opened.options.validate()?;
        Ok(opened)
    }
}

#[cfg(test)]
pub(crate) fn seal_test(runtime: &RuntimeContext, expected_revision: u64, no_save: bool) -> Args {
    let sample_root = crate::attachment::image_key::attach_root_for_db_dir(&runtime.config.db_dir);
    std::fs::create_dir_all(&sample_root).unwrap();
    let (_guard, sample_identity) =
        crate::attachment::image_key::pin_image_root(&sample_root).unwrap();
    seal_opened(&Opened {
        version: 1,
        purpose: PURPOSE.into(),
        runtime_id: runtime.id.clone(),
        config_sha256: ConfigPin::new(runtime).unwrap().fingerprint().unwrap(),
        expected_revision,
        options: ImportOptions {
            no_save,
            timeout: 5,
            max_mib: 1,
            sample_root: None,
        },
        material: ImageMaterial {
            aes: *b"syntheticAESkey1",
            xor: 42,
        },
        sample_root,
        sample_identity,
    })
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};
    fn runtime(root: &Path, name: &str) -> RuntimeContext {
        let path = root.join(format!("{name}.json"));
        let config = crate::config::Config {
            db_dir: root.join(name).join("db_storage"),
            keys_file: root.join(format!("{name}-identity.json")),
            key_store: Some(root.join(format!("{name}.dpapi"))),
            decrypted_dir: root.join(format!("{name}-decrypted")),
            wechat_process: "SyntheticNotRunning.exe".into(),
        };
        fs::create_dir_all(&config.db_dir).unwrap();
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        RuntimeContext::from_config(path, config, root.join("home")).unwrap()
    }
    #[test]
    fn sealed_envelope_binds_account_config_purpose_and_hides_material() {
        let root = tempfile::tempdir().unwrap();
        let a = runtime(root.path(), "a");
        let b = runtime(root.path(), "b");
        let args = seal_test(&a, 7, true);
        let opened = args.open(&a).unwrap();
        assert_eq!(opened.expected_revision, 7);
        assert!(opened.options.no_save);
        assert_eq!(opened.material.aes, *b"syntheticAESkey1");
        assert!(args.open(&b).is_err());
        assert_eq!(format!("{args:?}"), "ImageImport([REDACTED])");
        let wire = serde_json::to_string(&args).unwrap();
        assert!(
            !wire.contains("syntheticAESkey1")
                && !wire.contains("aes_key")
                && !wire.contains("expected_revision")
        );
        let mut wrong = args.open(&a).unwrap();
        wrong.purpose = "another-purpose".into();
        assert!(seal_opened(&wrong).unwrap().open(&a).is_err());
        wrong.purpose = PURPOSE.into();
        wrong.version = 2;
        assert!(seal_opened(&wrong).unwrap().open(&a).is_err());
        let mut config = a.config.clone();
        config.wechat_process = "OtherSynthetic.exe".into();
        fs::write(&a.config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(args.open(&a).is_err());
    }
    #[test]
    fn strict_plain_input_and_sealed_lengths_do_not_start_daemon_on_rejection() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path(), "a");
        fs::create_dir_all(crate::attachment::image_key::attach_root_for_db_dir(
            &runtime.config.db_dir,
        ))
        .unwrap();
        let options = ImportOptions {
            no_save: true,
            timeout: 5,
            max_mib: 1,
            sample_root: None,
        };
        for input in [
            b"".to_vec(),
            b"{\"aes_key\":\"sensitive-invalid\",\"xor_key\":2}".to_vec(),
            b"{\"aes_key\":\"11111111111111111111111111111111\",\"xor_key\":2,\"path\":\"secret\"}"
                .to_vec(),
            b"{\"aes_key\":\"11111111111111111111111111111111\",\"xor_key\":256}".to_vec(),
            b"{\"aes_key\":\"1111".to_vec(),
            vec![b'x'; MAX_INPUT_BYTES + 1],
        ] {
            let error = seal_stdin(&runtime, input.as_slice(), options.clone()).unwrap_err();
            let diagnostic = format!("{error:#}");
            assert!(
                !diagnostic.contains("sensitive-invalid")
                    && !diagnostic.contains("11111111")
                    && !diagnostic.contains("secret")
            );
        }
        assert!(serde_json::from_value::<Args>(
            serde_json::json!({"envelope":"a".repeat(MAX_ENVELOPE_BYTES+1)})
        )
        .is_err());
        assert!(serde_json::from_value::<Args>(
            serde_json::json!({"envelope":"AAAA","aes_key":"invalid"})
        )
        .is_err());
        let malformed = Args {
            envelope: "AAAA".into(),
        };
        assert!(malformed.open(&runtime).is_err());
    }
}
