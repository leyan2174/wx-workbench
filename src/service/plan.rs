//! Typed operations for the private worker channel; never public task input.
use super::{
    protocol::{Format, Kind, Options, Submission},
    settings::Settings,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    WechatKeys {
        config: PathBuf,
        authorize_memory_scan: bool,
    },
    WechatDecrypt {
        config: PathBuf,
    },
    ImageKey {
        config: PathBuf,
        authorize_memory_scan: bool,
        timeout: u64,
        max_mib: usize,
    },
    ExportMessages {
        config: PathBuf,
        output: PathBuf,
        users: Vec<String>,
        formats: Vec<Format>,
        include_images: bool,
        allow_missing_media: bool,
    },
    TranscribeChats {
        config: PathBuf,
        output: PathBuf,
        users: Vec<String>,
        allow_upload: bool,
    },
    DecodeImages {
        config: PathBuf,
        output: PathBuf,
    },
    SnsArchive {
        config: PathBuf,
        output: PathBuf,
    },
    SnsExport {
        config: PathBuf,
        output: PathBuf,
        users: Vec<String>,
        download_media: bool,
    },
    VoiceBatch {
        config: PathBuf,
        output: PathBuf,
        users: Vec<String>,
    },
    EnterpriseDecrypt {
        input: PathBuf,
        output: PathBuf,
        key_file: PathBuf,
    },
    EnterpriseBatchDecrypt {
        data_dir: PathBuf,
        output: PathBuf,
        keys: EnterpriseKeys,
    },
    EnterpriseExport {
        snapshot: PathBuf,
        output: PathBuf,
        selection: EnterpriseSelection,
    },
    EnterpriseDiscover {
        root: Option<PathBuf>,
    },
    EnterpriseScan {
        data_dir: PathBuf,
        pids: Vec<u32>,
        authorize_memory_scan: bool,
    },
    EnterpriseRun {
        data_dir: PathBuf,
        decrypted_output: PathBuf,
        export_output: PathBuf,
        keys: EnterpriseKeys,
        selection: EnterpriseSelection,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnterpriseKeys {
    pub key_file: Option<PathBuf>,
    pub keys_file: Option<PathBuf>,
    pub authorize_memory_scan: bool,
    pub pids: Vec<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnterpriseSelection {
    pub users: Vec<String>,
    pub formats: Vec<Format>,
    pub self_id: Option<i64>,
    pub all_conversations: bool,
}
pub fn capabilities(settings: &Settings) -> Vec<Value> {
    [Kind::WechatKeys, Kind::WechatDecrypt, Kind::ImageKey, Kind::ExportAll, Kind::DecodeImages, Kind::SnsDecrypt,
        Kind::WxworkDecrypt, Kind::WxworkExport, Kind::VoiceMp3, Kind::WxworkDiscover,
        Kind::WxworkScan, Kind::WxworkRun].into_iter().map(|kind| {
        let reason = match kind {
            Kind::WxworkDecrypt if settings.enterprise_input.is_none() && settings.enterprise_data_dir.is_none() => Some("需要固定 enterprise-data-dir 或单库 enterprise-input"),
            Kind::WxworkScan | Kind::WxworkRun if settings.enterprise_data_dir.is_none() => Some("需要固定 enterprise-data-dir"),
            Kind::WxworkExport if settings.enterprise_snapshot.is_none() => Some("需要启动参数 enterprise-snapshot"),
            _ => None,
        };
        let options: &[&str] = match kind {
            Kind::ExportAll => &["users", "formats", "include_voice", "include_sns", "include_sns_media",
                "include_images", "allow_missing_media", "with_transcriptions", "allow_upload"],
            Kind::WxworkExport => &["users", "formats", "all_conversations"],
            Kind::WxworkRun => &["users", "formats", "all_conversations", "authorize_memory_scan"],
            Kind::WxworkScan | Kind::ImageKey | Kind::WechatKeys => &["authorize_memory_scan"],
            Kind::WxworkDecrypt if settings.enterprise_data_dir.is_some() => &["authorize_memory_scan"],
            Kind::SnsDecrypt => &["users", "include_sns_media"],
            Kind::VoiceMp3 => &["users"],
            _ => &[],
        };
        json!({"kind":kind,"enabled":reason.is_none(),"reason":reason,"options":options,
            "formats":if matches!(kind,Kind::WxworkExport|Kind::WxworkRun|Kind::ExportAll) { vec!["json","csv","html"] } else {vec![]},
            "defaults":Options::default(),
            "requires_memory_consent":matches!(kind,Kind::WxworkScan|Kind::ImageKey|Kind::WechatKeys),
            "requires_explicit_scope":matches!(kind,Kind::WxworkExport|Kind::WxworkRun),
            "transcription_output":if kind==Kind::ExportAll {Some("separate_json")} else {None}})
    }).collect()
}

pub fn validate(request: &Submission, settings: &Settings) -> Result<()> {
    let o = &request.options;
    ensure!(o.users.len() <= 200, "最多选择 200 个会话");
    ensure!(
        o.users
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            == o.users.len(),
        "不能重复选择会话"
    );
    for user in &o.users {
        ensure!(
            !user.is_empty()
                && user.len() <= 256
                && !user.starts_with('-')
                && !user.chars().any(|c| c.is_control() || c == ','),
            "会话标识无效"
        );
    }
    ensure!(o.formats.len() <= 3, "格式数量超限");
    ensure!(
        !o.formats
            .iter()
            .enumerate()
            .any(|(i, f)| o.formats[..i].contains(f)),
        "不能重复选择格式"
    );
    ensure!(
        !o.include_sns_media || request.kind == Kind::SnsDecrypt || o.include_sns,
        "媒体下载必须属于朋友圈任务"
    );
    match request.kind {
        Kind::ImageKey | Kind::WechatKeys => {
            ensure!(o.authorize_memory_scan, "本次任务未授权读取微信进程内存")
        }
        Kind::WxworkExport => {
            ensure!(settings.enterprise_snapshot.is_some(), "未配置企业微信快照");
        }
        Kind::WxworkScan => {
            ensure!(
                settings.enterprise_data_dir.is_some(),
                "未配置企业微信 Data 目录"
            );
            ensure!(
                o.authorize_memory_scan,
                "本次任务未授权读取企业微信进程内存"
            );
        }
        Kind::WxworkRun | Kind::WxworkDecrypt => {
            if settings.enterprise_data_dir.is_some() {
                ensure!(
                    settings.enterprise_key_file.is_some()
                        || settings.enterprise_keys_file.is_some()
                        || o.authorize_memory_scan,
                    "缺少企业微信文件密钥，且本次任务未授权内存取钥"
                );
            } else {
                ensure!(
                    request.kind == Kind::WxworkDecrypt
                        && settings.enterprise_input.is_some()
                        && settings.enterprise_key_file.is_some(),
                    "未配置企业微信输入和密钥"
                );
                ensure!(!o.authorize_memory_scan, "单库离线模式不接受内存扫描授权");
            }
        }
        _ => (),
    }
    if request.kind != Kind::ExportAll {
        ensure!(!o.include_voice && !o.include_sns, "任务不支持组合导出");
        ensure!(
            o.include_images && !o.allow_missing_media && !o.with_transcriptions && !o.allow_upload,
            "任务不支持个人聊天媒体或转录选项"
        );
        ensure!(
            request.kind == Kind::SnsDecrypt || !o.include_sns_media,
            "任务不支持媒体下载"
        );
        ensure!(
            matches!(request.kind, Kind::WxworkExport | Kind::WxworkRun) || o.formats.is_empty(),
            "任务不支持格式选项"
        );
    }
    if matches!(
        request.kind,
        Kind::WechatDecrypt
            | Kind::WechatKeys
            | Kind::DecodeImages
            | Kind::WxworkDecrypt
            | Kind::WxworkDiscover
            | Kind::WxworkScan
            | Kind::ImageKey
    ) {
        ensure!(o.users.is_empty(), "此任务不接受会话筛选");
    }
    if matches!(request.kind, Kind::WxworkExport | Kind::WxworkRun) {
        ensure!(
            o.all_conversations == o.users.is_empty(),
            "须明确选择会话或确认全部会话，不能混用"
        );
    } else {
        ensure!(!o.all_conversations, "任务不支持企业微信全部会话选项");
    }
    ensure!(
        !o.authorize_memory_scan
            || matches!(
                request.kind,
                Kind::WxworkScan
                    | Kind::WxworkDecrypt
                    | Kind::WxworkRun
                    | Kind::ImageKey
                    | Kind::WechatKeys
            ),
        "该任务不接受内存扫描授权"
    );
    ensure!(
        !o.allow_upload || o.with_transcriptions,
        "上传授权必须属于本次转录任务"
    );
    if o.with_transcriptions {
        ensure!(
            matches!(
                settings.transcription_backend.as_str(),
                "openai" | "whisper_cpp" | "local"
            ),
            "需要明确配置 local、whisper_cpp 或 openai 转录后端"
        );
        ensure!(
            (settings.transcription_backend == "openai") == o.allow_upload,
            "云端转录须明确授权上传；本地转录不接受上传选项"
        );
    }
    Ok(())
}

fn formats(o: &Options) -> Vec<Format> {
    if o.formats.is_empty() {
        vec![Format::Json]
    } else {
        o.formats.clone()
    }
}
pub fn plan(
    request: &Submission,
    settings: &Settings,
    config: &Path,
    output: &Path,
) -> Result<Vec<Step>> {
    validate(request, settings)?;
    let o = &request.options;
    let config = config.to_path_buf();
    let sns = || Step::SnsExport {
        config: config.clone(),
        output: output.join("sns"),
        users: o.users.clone(),
        download_media: o.include_sns_media,
    };
    let voice = || Step::VoiceBatch {
        config: config.clone(),
        output: output.join("voice"),
        users: o.users.clone(),
    };
    let keys = || EnterpriseKeys {
        key_file: settings.enterprise_key_file.clone(),
        keys_file: settings.enterprise_keys_file.clone(),
        authorize_memory_scan: o.authorize_memory_scan,
        pids: if o.authorize_memory_scan {
            settings.enterprise_pids.clone()
        } else {
            Vec::new()
        },
    };
    let selection = || EnterpriseSelection {
        users: o.users.clone(),
        formats: formats(o),
        self_id: settings.enterprise_self_id,
        all_conversations: o.all_conversations,
    };
    let mut steps = Vec::new();
    match request.kind {
        Kind::WechatKeys => steps.push(Step::WechatKeys {
            config: config.clone(),
            authorize_memory_scan: true,
        }),
        Kind::WechatDecrypt => steps.push(Step::WechatDecrypt {
            config: config.clone(),
        }),
        Kind::ImageKey => steps.push(Step::ImageKey {
            config: config.clone(),
            authorize_memory_scan: true,
            timeout: 120,
            max_mib: 4096,
        }),
        Kind::ExportAll => {
            steps.push(Step::ExportMessages {
                config: config.clone(),
                output: output.join("chats"),
                users: o.users.clone(),
                formats: formats(o),
                include_images: o.include_images,
                allow_missing_media: o.allow_missing_media,
            });
            if o.with_transcriptions {
                steps.push(Step::TranscribeChats {
                    config: config.clone(),
                    output: output.join("transcribed-chats"),
                    users: o.users.clone(),
                    allow_upload: o.allow_upload,
                });
            }
            if o.include_sns {
                steps.push(sns());
            }
            if o.include_voice {
                steps.push(voice());
            }
        }
        Kind::DecodeImages => steps.push(Step::DecodeImages {
            config: config.clone(),
            output: output.join("images"),
        }),
        Kind::SnsDecrypt => {
            steps.push(Step::SnsArchive {
                config: config.clone(),
                output: output.join("sns-cache"),
            });
            steps.push(sns());
        }
        Kind::VoiceMp3 => steps.push(voice()),
        Kind::WxworkDecrypt => {
            if let Some(data_dir) = &settings.enterprise_data_dir {
                steps.push(Step::EnterpriseBatchDecrypt {
                    data_dir: data_dir.clone(),
                    output: output.join("enterprise-snapshot"),
                    keys: keys(),
                });
            } else {
                steps.push(Step::EnterpriseDecrypt {
                    input: settings.enterprise_input.clone().expect("validated input"),
                    output: output.join("enterprise.db"),
                    key_file: settings.enterprise_key_file.clone().expect("validated key"),
                });
            }
        }
        Kind::WxworkExport => steps.push(Step::EnterpriseExport {
            snapshot: settings
                .enterprise_snapshot
                .clone()
                .expect("validated snapshot"),
            output: output.join("enterprise-export"),
            selection: selection(),
        }),
        Kind::WxworkDiscover => steps.push(Step::EnterpriseDiscover {
            root: settings.enterprise_discovery_root.clone(),
        }),
        Kind::WxworkScan => steps.push(Step::EnterpriseScan {
            data_dir: settings
                .enterprise_data_dir
                .clone()
                .expect("validated data"),
            pids: settings.enterprise_pids.clone(),
            authorize_memory_scan: true,
        }),
        Kind::WxworkRun => steps.push(Step::EnterpriseRun {
            data_dir: settings
                .enterprise_data_dir
                .clone()
                .expect("validated data"),
            decrypted_output: output.join("enterprise-snapshot"),
            export_output: output.join("enterprise-export"),
            keys: keys(),
            selection: selection(),
        }),
    }
    Ok(steps)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_injection_and_missing_consent() {
        for user in ["--config=other", "a,b", "a\nb", ""] {
            let r = Submission {
                kind: Kind::ExportAll,
                options: Options {
                    users: vec![user.into()],
                    ..Default::default()
                },
            };
            assert!(validate(&r, &Settings::default()).is_err());
        }
        for kind in [Kind::WechatKeys, Kind::ImageKey, Kind::WxworkScan] {
            assert!(validate(
                &Submission {
                    kind,
                    options: Options::default()
                },
                &Settings::default()
            )
            .is_err());
        }
    }
    #[test]
    fn combined_plan_preserves_order_and_typed_parameters() {
        let settings = Settings {
            transcription_backend: "local".into(),
            ..Default::default()
        };
        let r = Submission {
            kind: Kind::ExportAll,
            options: Options {
                include_voice: true,
                include_sns: true,
                with_transcriptions: true,
                ..Default::default()
            },
        };
        let steps = plan(&r, &settings, Path::new("config.json"), Path::new("out")).unwrap();
        assert_eq!(steps.len(), 4);
        assert!(
            matches!(&steps[0], Step::ExportMessages { formats, output, .. } if formats == &[Format::Json] && output == &PathBuf::from("out").join("chats"))
        );
        assert!(matches!(
            &steps[1],
            Step::TranscribeChats {
                allow_upload: false,
                ..
            }
        ));
        assert!(matches!(
            &steps[2],
            Step::SnsExport {
                download_media: false,
                ..
            }
        ));
        assert!(matches!(&steps[3], Step::VoiceBatch { .. }));
        let bytes = serde_json::to_vec(&steps).unwrap();
        assert_eq!(
            serde_json::to_vec(&serde_json::from_slice::<Vec<Step>>(&bytes).unwrap()).unwrap(),
            bytes
        );
        assert!(serde_json::from_value::<Step>(
            json!({"operation":"wechat_decrypt","config":"x","argv":["--evil"]})
        )
        .is_err());
    }
    #[test]
    fn enterprise_scope_and_cloud_consent_are_explicit() {
        let mut settings = Settings {
            enterprise_snapshot: Some("snapshot".into()),
            transcription_backend: "openai".into(),
            ..Default::default()
        };
        let mut r = Submission {
            kind: Kind::WxworkExport,
            options: Options::default(),
        };
        assert!(validate(&r, &settings).is_err());
        r.options.all_conversations = true;
        assert!(validate(&r, &settings).is_ok());
        r.kind = Kind::ExportAll;
        r.options.all_conversations = false;
        r.options.with_transcriptions = true;
        assert!(validate(&r, &settings).is_err());
        r.options.allow_upload = true;
        assert!(validate(&r, &settings).is_ok());
        settings.transcription_backend = "local".into();
        assert!(validate(&r, &settings).is_err());
    }
}
