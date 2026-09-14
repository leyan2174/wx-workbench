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
}
pub fn capabilities() -> Vec<Value> {
    [
        Kind::WechatKeys,
        Kind::WechatDecrypt,
        Kind::ImageKey,
        Kind::ExportAll,
        Kind::DecodeImages,
        Kind::SnsDecrypt,
        Kind::VoiceMp3,
    ]
    .into_iter()
    .map(|kind| {
        let options: &[&str] = match kind {
            Kind::ExportAll => &[
                "users",
                "formats",
                "include_voice",
                "include_sns",
                "include_sns_media",
                "include_images",
                "allow_missing_media",
                "with_transcriptions",
                "allow_upload",
            ],
            Kind::ImageKey | Kind::WechatKeys => &["authorize_memory_scan"],
            Kind::SnsDecrypt => &["users", "include_sns_media"],
            Kind::VoiceMp3 => &["users"],
            _ => &[],
        };
        json!({"kind":kind,"enabled":true,"reason":null,"options":options,
            "formats":if kind == Kind::ExportAll { vec!["json","csv","html"] } else {vec![]},
            "defaults":Options::default(),
            "requires_memory_consent":matches!(kind,Kind::ImageKey|Kind::WechatKeys),
            "transcription_output":if kind==Kind::ExportAll {Some("separate_json")} else {None}})
    })
    .collect()
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
        ensure!(o.formats.is_empty(), "任务不支持格式选项");
    }
    if matches!(
        request.kind,
        Kind::WechatDecrypt | Kind::WechatKeys | Kind::DecodeImages | Kind::ImageKey
    ) {
        ensure!(o.users.is_empty(), "此任务不接受会话筛选");
    }
    ensure!(
        !o.authorize_memory_scan || matches!(request.kind, Kind::ImageKey | Kind::WechatKeys),
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
        for kind in [Kind::WechatKeys, Kind::ImageKey] {
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
    fn cloud_consent_is_explicit() {
        let mut settings = Settings {
            transcription_backend: "openai".into(),
            ..Default::default()
        };
        let mut r = Submission {
            kind: Kind::ExportAll,
            options: Options::default(),
        };
        r.options.with_transcriptions = true;
        assert!(validate(&r, &settings).is_err());
        r.options.allow_upload = true;
        assert!(validate(&r, &settings).is_ok());
        settings.transcription_backend = "local".into();
        assert!(validate(&r, &settings).is_err());
    }
}
