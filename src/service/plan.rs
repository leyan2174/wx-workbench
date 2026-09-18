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
    ChatPlan {
        config: PathBuf,
        output: PathBuf,
        request: super::chat_plan::Request,
    },
    ChatPlanReview {
        config: PathBuf,
        output: PathBuf,
        request: super::chat_plan::ReviewRequest,
    },
    ChatPlanApply {
        config: PathBuf,
        output: PathBuf,
        request: super::chat_plan::ApplyRequest,
        selected_sha256: Option<String>,
    },
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
        dry_run: bool,
        max_media_bytes: Option<u64>,
        max_total_media_bytes: Option<u64>,
    },
    ExportHistory {
        config: PathBuf,
        output: PathBuf,
        request: super::history_export::Request,
        since_ts: Option<i64>,
        until_ts: Option<i64>,
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
}
pub fn capabilities() -> Vec<Value> {
    [
        Kind::WechatKeys,
        Kind::WechatDecrypt,
        Kind::ImageKey,
        Kind::ExportAll,
        Kind::ExportHistory,
        Kind::ChatPlan,
        Kind::ChatPlanReview,
        Kind::ChatPlanApply,
        Kind::DecodeImages,
        Kind::SnsDecrypt,
    ]
    .into_iter()
    .map(|kind| {
        let options: &[&str] = match kind {
            Kind::ExportAll => &[
                "users",
                "formats",
                "include_sns",
                "include_sns_media",
                "include_images",
                "allow_missing_media",
                "dry_run",
                "max_media_bytes",
                "max_total_media_bytes",
            ],
            Kind::ImageKey | Kind::WechatKeys => &["authorize_memory_scan"],
            Kind::ExportHistory => &["history_export"],
            Kind::ChatPlan => &["chat_plan"],
            Kind::ChatPlanReview => &["chat_plan_review"],
            Kind::ChatPlanApply => &["chat_plan_apply"],
            Kind::SnsDecrypt => &["users", "include_sns_media"],
            _ => &[],
        };
        json!({"kind":kind,"enabled":true,"reason":null,"options":options,
            "formats":match kind { Kind::ExportAll => vec!["json","csv","html"], Kind::ExportHistory => vec!["markdown","txt","json","yaml"], _ => vec![] },
            "defaults":Options::default(),
            "requires_memory_consent":matches!(kind,Kind::ImageKey|Kind::WechatKeys)})
    })
    .collect()
}

pub fn validate(request: &Submission, _settings: &Settings) -> Result<()> {
    let o = &request.options;
    ensure!(
        o.chat_plan.is_none() || request.kind == Kind::ChatPlan,
        "Unexpected plan request"
    );
    ensure!(
        o.chat_plan_review.is_none() || request.kind == Kind::ChatPlanReview,
        "Unexpected plan review"
    );
    ensure!(
        o.chat_plan_apply.is_none() || request.kind == Kind::ChatPlanApply,
        "Unexpected plan apply"
    );
    if matches!(
        request.kind,
        Kind::ChatPlan | Kind::ChatPlanReview | Kind::ChatPlanApply
    ) {
        ensure!(
            o.history_export.is_none()
                && o.users.is_empty()
                && o.formats.is_empty()
                && !o.include_sns
                && !o.include_sns_media
                && o.include_images
                && !o.allow_missing_media
                && !o.authorize_memory_scan
                && !o.dry_run
                && o.max_media_bytes.is_none()
                && o.max_total_media_bytes.is_none(),
            "Unsupported plan options"
        );
        match request.kind {
            Kind::ChatPlan => o
                .chat_plan
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Missing plan request"))?
                .validate()?,
            Kind::ChatPlanReview => o
                .chat_plan_review
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Missing plan review"))?
                .validate()?,
            Kind::ChatPlanApply => o
                .chat_plan_apply
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Missing plan apply"))?
                .validate()?,
            _ => unreachable!(),
        }
        return Ok(());
    }
    if request.kind == Kind::ExportHistory {
        let history = o
            .history_export
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing history export selection"))?;
        history.resolved_window()?;
        ensure!(
            o.users.is_empty()
                && o.formats.is_empty()
                && !o.include_sns
                && !o.include_sns_media
                && o.include_images
                && !o.allow_missing_media
                && !o.authorize_memory_scan
                && !o.dry_run
                && o.max_media_bytes.is_none()
                && o.max_total_media_bytes.is_none(),
            "Unsupported history export options"
        );
        return Ok(());
    }
    ensure!(
        o.history_export.is_none(),
        "Task does not accept history export selection"
    );
    use super::task_artifacts::{
        DEFAULT_MEDIA_BYTES, DEFAULT_TOTAL_MEDIA_BYTES, MAX_TOTAL_MEDIA_BYTES,
    };
    let has_budget = o.max_media_bytes.is_some() || o.max_total_media_bytes.is_some();
    ensure!(
        request.kind == Kind::ExportAll || (!o.dry_run && !has_budget),
        "Unsupported export options"
    );
    ensure!(
        !has_budget || o.include_images,
        "Media budgets require media export"
    );
    ensure!(
        !(o.dry_run && o.include_sns),
        "SNS export does not support dry run"
    );
    if request.kind == Kind::ExportAll {
        let single = o.max_media_bytes.unwrap_or(DEFAULT_MEDIA_BYTES);
        let total = o.max_total_media_bytes.unwrap_or(DEFAULT_TOTAL_MEDIA_BYTES);
        ensure!(
            (1..=500 * 1024 * 1024).contains(&single),
            "Invalid media byte budget"
        );
        ensure!(
            total >= single && total <= MAX_TOTAL_MEDIA_BYTES,
            "Invalid total media byte budget"
        );
    }
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
        ensure!(!o.include_sns, "任务不支持组合导出");
        ensure!(
            o.include_images && !o.allow_missing_media,
            "任务不支持个人聊天媒体选项"
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
    let mut steps = Vec::new();
    match request.kind {
        Kind::ChatPlan => steps.push(Step::ChatPlan {
            config,
            output: output.to_owned(),
            request: o.chat_plan.clone().unwrap(),
        }),
        Kind::ChatPlanReview => steps.push(Step::ChatPlanReview {
            config,
            output: output.to_owned(),
            request: o.chat_plan_review.clone().unwrap(),
        }),
        Kind::ChatPlanApply => steps.push(Step::ChatPlanApply {
            config,
            output: output.to_owned(),
            request: o.chat_plan_apply.clone().unwrap(),
            selected_sha256: None,
        }),
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
                dry_run: o.dry_run,
                max_media_bytes: o.max_media_bytes,
                max_total_media_bytes: o.max_total_media_bytes,
            });
            if o.include_sns {
                steps.push(sns());
            }
        }
        Kind::ExportHistory => {
            let request = o
                .history_export
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Missing history export selection"))?;
            let (since_ts, until_ts) = request.resolved_window()?;
            steps.push(Step::ExportHistory {
                config,
                output: output
                    .join("history")
                    .join(format!("history.{}", request.format.extension())),
                request,
                since_ts,
                until_ts,
            });
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
    }
    Ok(steps)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_options_preserve_legacy_signatures_and_enforce_budgets() {
        let legacy = json!({"users":[],"formats":[],"include_sns":false,"include_sns_media":false,
            "include_images":true,"allow_missing_media":false,"authorize_memory_scan":false});
        assert_eq!(serde_json::to_value(Options::default()).unwrap(), legacy);
        let mut request = Submission {
            kind: Kind::ExportAll,
            options: Options::default(),
        };
        request.options.max_media_bytes = Some(1);
        request.options.max_total_media_bytes = Some(1);
        request.options.dry_run = true;
        assert!(validate(&request, &Settings::default()).is_ok());
        let steps = plan(
            &request,
            &Settings::default(),
            Path::new("config"),
            Path::new("out"),
        )
        .unwrap();
        assert!(matches!(
            &steps[0],
            Step::ExportMessages {
                dry_run: true,
                max_media_bytes: Some(1),
                max_total_media_bytes: Some(1),
                ..
            }
        ));
        request.options.max_media_bytes = Some(0);
        assert!(validate(&request, &Settings::default()).is_err());
        request.options.max_media_bytes = Some(2);
        assert!(validate(&request, &Settings::default()).is_err());
        request.options.max_media_bytes = None;
        request.options.max_total_media_bytes =
            Some(super::super::task_artifacts::MAX_TOTAL_MEDIA_BYTES + 1);
        assert!(validate(&request, &Settings::default()).is_err());
        request.options.max_total_media_bytes = None;
        request.options.include_sns = true;
        assert!(validate(&request, &Settings::default()).is_err());
        request.options.include_sns = false;
        request.kind = Kind::WechatDecrypt;
        assert!(validate(&request, &Settings::default()).is_err());
        request.kind = Kind::ExportAll;
        request.options.include_images = false;
        request.options.max_media_bytes = Some(1);
        assert!(validate(&request, &Settings::default()).is_err());
    }
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
        let settings = Settings::default();
        let r = Submission {
            kind: Kind::ExportAll,
            options: Options {
                include_sns: true,
                ..Default::default()
            },
        };
        let steps = plan(&r, &settings, Path::new("config.json"), Path::new("out")).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(
            matches!(&steps[0], Step::ExportMessages { formats, output, .. } if formats == &[Format::Json] && output == &PathBuf::from("out").join("chats"))
        );
        assert!(matches!(
            &steps[1],
            Step::SnsExport {
                download_media: false,
                ..
            }
        ));
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
}
