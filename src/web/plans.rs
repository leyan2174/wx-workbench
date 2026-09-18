//! Plan reference reads and this Web process's scan permission.
use super::*;
use crate::service::{
    chat_plan::{Mode, PlanRef, SizeMode},
    protocol::{Kind, Task},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Page {
    sha256: String,
    #[serde(default)]
    plan_mode: Mode,
    #[serde(default)]
    offset: u64,
    #[serde(default = "page_limit")]
    limit: u32,
}
fn page_limit() -> u32 {
    50
}

pub(super) fn capabilities(mut kinds: Vec<Value>, enabled: bool, scan: bool) -> Vec<Value> {
    kinds.retain(|kind| {
        enabled
            || !matches!(
                kind["kind"].as_str(),
                Some("chat_plan" | "chat_plan_review" | "chat_plan_apply")
            )
    });
    for kind in &mut kinds {
        if kind["kind"] == "chat_plan" {
            kind["size_modes"] = if scan {
                json!(["estimate", "scan"])
            } else {
                json!(["estimate"])
            };
        }
    }
    kinds
}

pub(super) fn requires_scan(task: &Submission) -> bool {
    task.kind == Kind::ChatPlan
        && task
            .options
            .chat_plan
            .as_ref()
            .is_some_and(|request| matches!(request.size_mode, SizeMode::Scan))
}

// A denied host may retrieve an already accepted exact replay, never submit it anew.
pub(super) async fn existing_scan(
    state: &Shared,
    id: &str,
    request: &Submission,
) -> std::result::Result<Value, ApiError> {
    let existing = state
        .backend(Call::Get { id: id.into() })
        .await
        .map_err(|error| {
            if error
                .downcast_ref::<crate::service::protocol::ServiceError>()
                .is_some_and(|error| error.code == "not_found")
            {
                ApiError(StatusCode::FORBIDDEN, "plan_scan_not_authorized")
            } else {
                backend_error(error)
            }
        })?;
    let task: Task = serde_json::from_value(existing.clone()).map_err(unavailable)?;
    let saved = Submission {
        kind: task.kind,
        options: task.options,
    };
    if serde_json::to_value(&saved).map_err(unavailable)?
        != serde_json::to_value(request).map_err(unavailable)?
    {
        return Err(ApiError(StatusCode::CONFLICT, "提交 ID 已被不同任务使用"));
    }
    Ok(existing)
}

pub(super) async fn read(
    State(state): State<Arc<Shared>>,
    Path((task_id, artifact_id)): Path<(String, String)>,
    query: std::result::Result<Query<Page>, axum::extract::rejection::QueryRejection>,
) -> ApiResult {
    let Query(page) = query.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_page"))?;
    if !(1..=100).contains(&page.limit) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_page"));
    }
    let plan_ref = PlanRef {
        task_id,
        artifact_id,
        sha256: page.sha256,
    };
    plan_ref
        .validate()
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "plan_selection_invalid"))?;
    state
        .backend(Call::ReadChatPlan {
            plan_ref,
            plan_mode: page.plan_mode,
            offset: page.offset,
            limit: page.limit,
        })
        .await
        .map(Json)
        .map_err(backend_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_scan_permission_does_not_change_other_kinds() {
        let kinds = vec![
            json!({"kind":"export_history"}),
            json!({"kind":"chat_plan"}),
            json!({"kind":"chat_plan_apply"}),
        ];
        let denied = capabilities(kinds.clone(), true, false);
        assert_eq!(denied[1]["size_modes"], json!(["estimate"]));
        assert_eq!(denied[0], kinds[0]);
        assert_eq!(denied[2], kinds[2]);
        assert_eq!(
            capabilities(kinds.clone(), true, true)[1]["size_modes"],
            json!(["estimate", "scan"])
        );
        assert_eq!(capabilities(kinds, false, true).len(), 1);
    }
    #[test]
    fn scan_request_is_detected_without_changing_shared_settings() {
        let make = |mode| {
            serde_json::from_value::<Submission>(
                json!({"kind":"chat_plan","options":{"chat_plan":{"size_mode":mode}}}),
            )
            .unwrap()
        };
        assert!(requires_scan(&make("scan")));
        assert!(!requires_scan(&make("estimate")));
    }

    #[test]
    fn startup_permission_cannot_be_injected_into_business_or_shared_settings() {
        assert!(serde_json::from_value::<Submission>(json!({
            "kind":"chat_plan","options":{"chat_plan":{"allow_plan_scan":true}}
        }))
        .is_err());
        assert!(serde_json::from_value::<Submission>(json!({
            "kind":"chat_plan","options":{"chat_plan":{},"allow_plan_scan":true}
        }))
        .is_err());
        assert!(serde_json::from_value::<SettingsInput>(json!({"allow_plan_scan":true})).is_err());
    }
    #[test]
    fn plan_read_query_is_bounded_and_not_a_path_api() {
        let hash = "a".repeat(64);
        let uri = format!("/plan?sha256={hash}").parse().unwrap();
        let Query(page) = Query::<Page>::try_from_uri(&uri).unwrap();
        assert_eq!(page.limit, 50);
        assert_eq!(page.offset, 0);
        for suffix in [
            "&path=private",
            "&offset=-1",
            "&offset=0&offset=1",
            "&limit=1.5",
            "&plan_mode=other",
        ] {
            let uri = format!("/plan?sha256={hash}{suffix}").parse().unwrap();
            assert!(Query::<Page>::try_from_uri(&uri).is_err());
        }
    }
}
