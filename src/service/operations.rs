//! Typed daemon capabilities, without raw command lines or arbitrary process execution.
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(
    tag = "kind",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Operation {
    FirstRunCheck,
    MigrateKeys {
        args: crate::service::operation_requests::key_migration::Args,
    },
    Extract {
        attachment_id: String,
        output: String,
        overwrite: bool,
        json: bool,
    },
    TranscribeAudio {
        args: crate::service::operation_requests::asr::TranscribeAudioNativeArgs,
    },
    TranscribeChat {
        args: crate::service::operation_requests::asr::TranscribeChatNativeArgs,
    },
    TranscribeBatch {
        args: crate::service::operation_requests::asr_batch::Args,
    },
    TranscribeDatabase {
        args: crate::service::operation_requests::asr_database::TranscribeDatabaseNativeArgs,
    },
    ChatPlan {
        args: crate::service::operation_requests::chat_plan::Args,
    },
    Cleanup {
        args: crate::service::operation_requests::cleanup_native::Args,
    },
    DatabaseKeys {
        args: crate::service::operation_requests::database_keys::Args,
    },
    Export {
        chat: String,
        since: Option<String>,
        until: Option<String>,
        limit: usize,
        format: String,
        output: Option<String>,
        opts: crate::service::output::OutputOpts,
    },
    ExportChat {
        chat: String,
        output: PathBuf,
    },
    ExportChats {
        args: crate::service::operation_requests::export_chats::Args,
    },
    ExportDelta {
        args: crate::service::operation_requests::export_delta::Args,
    },
    ExportMessages {
        args: crate::service::operation_requests::export_messages::Args,
    },
    ImageKeys {
        args: crate::service::operation_requests::image_keys::Args,
    },
    ImageKeyMonitor {
        args: crate::service::operation_requests::image_keys::MonitorArgs,
    },
    Initialize {
        force: bool,
        db_dir_override: Option<String>,
        #[serde(with = "key_provider")]
        provider: crate::service::operation_requests::key_provider::KeyProvider,
        restart: bool,
        executable: Option<std::path::PathBuf>,
        timeout: u64,
    },
    Monitor {
        args: crate::service::operation_requests::monitor_native::Args,
    },
    Latency {
        args: crate::service::operation_requests::monitor_native::LatencyArgs,
    },
    NewMessages {
        limit: usize,
        opts: crate::service::output::OutputOpts,
    },
    Setup {
        args: crate::service::operation_requests::setup_native::Args,
    },
    SetupPreview {
        args: crate::service::operation_requests::setup_native::Args,
    },
    SetupApply {
        args: crate::service::operation_requests::setup_native::Args,
        review: SetupReview,
    },
    SnsAlbum {
        args: crate::service::operation_requests::sns_album::Args,
    },
    SnsArchive {
        args: crate::service::operation_requests::sns_archive::Args,
    },
    SnsTimeline {
        args: crate::service::operation_requests::sns_timeline::Args,
    },
    Voices {
        chat: Option<String>,
        output: String,
        limit: Option<usize>,
        offset: usize,
        since: Option<String>,
        until: Option<String>,
        overwrite: bool,
        json_output: bool,
    },
    Toolkit {
        operation: ToolkitOperation,
    },
    ExportAll {
        args: crate::service::operation_requests::export_all::Args,
        prepare: bool,
        announce: bool,
    },
    PreparedEmoticons {
        args: crate::service::operation_requests::export_emoticons::Args,
    },
    PreparedDecrypt {
        incremental: bool,
        dry_run: bool,
    },
    RunStatus {
        exported_dir: Option<PathBuf>,
        json: bool,
    },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SetupReview {
    pub config_path: PathBuf,
    pub snapshot_sha256: Option<String>,
    pub arguments_sha256: String,
}

impl Operation {
    /// Successful mutations which can invalidate cached database keys or configuration.
    pub fn invalidates_query(&self) -> bool {
        match self {
            Self::Initialize { .. }
            | Self::MigrateKeys { .. }
            | Self::DatabaseKeys { .. }
            | Self::PreparedDecrypt { .. }
            | Self::PreparedEmoticons { .. } => true,
            Self::ImageKeys { args } => !args.no_save,
            Self::ImageKeyMonitor { args } => args.saves_keys(),
            Self::ExportAll { args, prepare, .. } => *prepare && !args.dry_run,
            Self::Setup { args } => args.apply && !args.check,
            Self::SetupApply { .. } => true,
            Self::Cleanup { args } => args.execute,
            _ => false,
        }
    }

    /// Pure protocol/authorization validation before account discovery or daemon startup.
    pub fn validate_request(&self) -> anyhow::Result<()> {
        crate::service::operation_requests::validation::validate(self)
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(
    tag = "kind",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ToolkitOperation {
    DecodeSnsVideo {
        input: PathBuf,
        output: PathBuf,
        key_file: Option<PathBuf>,
        wasm: Option<PathBuf>,
    },
    ExportSnsNative {
        sns_db: PathBuf,
        output_dir: PathBuf,
        contact_db: Option<PathBuf>,
        contacts: Option<String>,
        utc_offset: Option<String>,
        download_media: bool,
        update: bool,
        adopt_existing: bool,
        local_cache: crate::service::operation_requests::export_sns::LocalCacheArgs,
    },
    ExportEmoticons(crate::service::operation_requests::export_emoticons::Args),
    Status {
        json: bool,
    },
    Decrypt {
        incremental: bool,
        dry_run: bool,
    },
    DecodeImages {
        attach_dir: Option<String>,
        decoded_dir: Option<String>,
        aes_key: Option<String>,
        xor_key: Option<String>,
        force: bool,
    },
    DecodeImage {
        dat_file: String,
        output_file: Option<String>,
    },
    BatchDecryptImages {
        input_dir: String,
        output_dir: Option<String>,
    },
    VoiceBatch {
        config: PathBuf,
        output_dir: Option<PathBuf>,
        contacts: Option<String>,
    },
    VoiceToMp3 {
        input: String,
        output: Option<String>,
    },
}
pub mod key_provider {
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize)]
    #[serde(
        remote = "crate::service::operation_requests::key_provider::KeyProvider",
        rename_all = "snake_case"
    )]
    pub enum Wire {
        Auto,
        Memory,
        Account,
    }
    pub fn serialize<S: serde::Serializer>(
        value: &crate::service::operation_requests::key_provider::KeyProvider,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Wire::serialize(value, serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<crate::service::operation_requests::key_provider::KeyProvider, D::Error> {
        Wire::deserialize(deserializer)
    }
}
pub mod plan_mode {
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize)]
    #[serde(
        remote = "crate::service::operation_requests::plan::Mode",
        rename_all = "snake_case"
    )]
    pub enum Wire {
        Blacklist,
        Whitelist,
    }
    pub fn serialize<S: serde::Serializer>(
        value: &crate::service::operation_requests::plan::Mode,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Wire::serialize(value, serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<crate::service::operation_requests::plan::Mode, D::Error> {
        Wire::deserialize(deserializer)
    }
}
pub mod optional_plan_mode {
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize)]
    struct Wrapped(
        #[serde(with = "super::plan_mode")] crate::service::operation_requests::plan::Mode,
    );
    pub fn serialize<S: serde::Serializer>(
        value: &Option<crate::service::operation_requests::plan::Mode>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(Wrapped).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<crate::service::operation_requests::plan::Mode>, D::Error> {
        Ok(Option::<Wrapped>::deserialize(deserializer)?.map(|v| v.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_a_typed_bootstrap_safe_operation() {
        fn traits<T: Clone + std::fmt::Debug + serde::Serialize + serde::de::DeserializeOwned>() {}
        traits::<Operation>();
        let operation = Operation::Toolkit {
            operation: ToolkitOperation::Status { json: true },
        };
        let value = serde_json::to_value(operation.clone()).unwrap();
        assert_eq!(value["kind"], "toolkit");
        assert_eq!(value["args"]["operation"]["kind"], "status");
        let decoded: Operation = serde_json::from_value(value).unwrap();
        assert!(matches!(
            decoded,
            Operation::Toolkit {
                operation: ToolkitOperation::Status { json: true }
            }
        ));
    }

    #[test]
    fn unknown_commands_and_arbitrary_argv_are_rejected() {
        for value in [
            serde_json::json!({"kind":"run","args":{"command":"cmd.exe","argv":["/c","echo"]}}),
            serde_json::json!({"kind":"toolkit","args":{"operation":{"kind":"run","args":{"command":"anything"}}}}),
            serde_json::json!({"kind":"toolkit","args":{"operation":{"kind":"status","args":{"json":true,"argv":[]}}}}),
        ] {
            assert!(serde_json::from_value::<Operation>(value).is_err());
        }
    }

    #[test]
    fn initialization_roundtrips_provider_and_paths_without_executing() {
        let operation = Operation::Initialize {
            force: true,
            db_dir_override: Some("C:/fixture/db_storage".into()),
            provider: crate::service::operation_requests::key_provider::KeyProvider::Account,
            restart: true,
            executable: Some("C:/fixture/Weixin.exe".into()),
            timeout: 300,
        };
        let serialized = serde_json::to_value(operation).unwrap();
        assert_eq!(serialized["args"]["provider"], "account");
        assert!(matches!(
            serde_json::from_value::<Operation>(serialized).unwrap(),
            Operation::Initialize {
                provider: crate::service::operation_requests::key_provider::KeyProvider::Account,
                timeout: 300,
                ..
            }
        ));
    }

    #[test]
    fn authorization_and_semantic_checks_do_not_need_an_account() {
        use crate::service::operation_requests::{asr, database_keys, export_delta};
        let no_scan = Operation::DatabaseKeys {
            args: database_keys::Args {
                authorize_memory_scan: false,
            },
        };
        assert!(no_scan
            .validate_request()
            .unwrap_err()
            .to_string()
            .contains("authorize-memory-scan"));
        let no_upload = Operation::TranscribeAudio {
            args: asr::TranscribeAudioNativeArgs {
                input: "must-not-read.silk".into(),
                backend: asr::BackendArgs {
                    backend: asr::BackendKind::ExplicitOpenAi,
                    api_key_file: Some("must-not-read.key".into()),
                    ..Default::default()
                },
            },
        };
        assert!(no_upload
            .validate_request()
            .unwrap_err()
            .to_string()
            .contains("allow-upload"));
        let invalid_date = Operation::ExportDelta {
            args: export_delta::Args {
                output: "must-not-create".into(),
                users: None,
                start: "not-a-timestamp".into(),
                end: None,
                run_id: None,
                append_run: false,
            },
        };
        assert!(invalid_date.validate_request().is_err());
        assert!(Operation::Toolkit {
            operation: ToolkitOperation::Status { json: true }
        }
        .validate_request()
        .is_ok());
    }
}
