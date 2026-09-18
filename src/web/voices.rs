//! This Web process's raw-voice write gate. Artifact reads are independent.
use super::*;
use crate::service::protocol::Task;

pub(super) fn capabilities(
    mut kinds: Vec<Value>,
    supported: bool,
    media_write: bool,
) -> Vec<Value> {
    if !supported || !media_write {
        kinds.retain(|kind| kind["kind"] != "export_voices");
    }
    kinds
}

fn replay(existing: Value, id: &str, request: &Submission) -> std::result::Result<Value, ApiError> {
    let task: Task = serde_json::from_value(existing.clone()).map_err(unavailable)?;
    if task.id != id {
        return Err(unavailable(anyhow::anyhow!(
            "Unexpected voice replay identity"
        )));
    }
    let saved = Submission {
        kind: task.kind,
        options: task.options,
    };
    // Typed deserialization supplies every default before comparing the full request.
    if serde_json::to_value(&saved).map_err(unavailable)?
        != serde_json::to_value(request).map_err(unavailable)?
    {
        return Err(ApiError(StatusCode::CONFLICT, "提交 ID 已被不同任务使用"));
    }
    Ok(existing)
}

// Never call Submit here: an evicted task must not be recreated without write permission.
pub(super) async fn existing(
    state: &Shared,
    id: &str,
    request: &Submission,
) -> std::result::Result<Value, ApiError> {
    let task = state
        .backend(Call::Get { id: id.into() })
        .await
        .map_err(|error| {
            if error
                .downcast_ref::<crate::service::protocol::ServiceError>()
                .is_some_and(|error| error.code == "not_found")
            {
                ApiError(StatusCode::FORBIDDEN, "media_write_not_authorized")
            } else {
                backend_error(error)
            }
        })?;
    replay(task, id, request)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submission(options: Value) -> Submission {
        serde_json::from_value(json!({"kind":"export_voices","options":options})).unwrap()
    }

    fn task(options: Value) -> Value {
        json!({"id":"saved","kind":"export_voices","options":options,"status":"succeeded",
            "created_at":0,"started_at":null,"finished_at":null,"exit_code":0,"logs":[],
            "log_start_seq":0,"next_log_seq":0,"output_dir":"synthetic","error":null})
    }

    #[test]
    fn write_gate_defaults_closed_and_preserves_other_capabilities() {
        assert!(!crate::service::web::HostSettings::default().allow_media_write);
        let kinds = vec![
            json!({"kind":"export_all"}),
            json!({"kind":"export_voices"}),
        ];
        assert_eq!(capabilities(kinds.clone(), true, false).len(), 1);
        assert_eq!(capabilities(kinds.clone(), false, true).len(), 1);
        assert_eq!(capabilities(kinds, true, true).len(), 2);
    }

    #[test]
    fn normalized_replay_preserves_all_fields_and_zero_is_not_none() {
        let saved = task(json!({"voice_export":{"chat":" exact "}}));
        let same = submission(
            json!({"voice_export":{"chat":" exact ","since":null,"until":null,"limit":null,"offset":0,"overwrite":false}}),
        );
        assert!(replay(saved.clone(), "saved", &same).is_ok());
        for changed in [
            json!({"chat":"exact"}),
            json!({"chat":null}),
            json!({"chat":" exact ","limit":0}),
            json!({"chat":" exact ","offset":1}),
            json!({"chat":" exact ","since":"2026-09-18"}),
            json!({"chat":" exact ","until":"2026-09-18"}),
            json!({"chat":" exact ","overwrite":true}),
        ] {
            let request = submission(json!({"voice_export":changed}));
            assert_eq!(
                replay(saved.clone(), "saved", &request).err().unwrap().0,
                StatusCode::CONFLICT
            );
        }
        let unrelated = submission(json!({"voice_export":{"chat":" exact "},"dry_run":true}));
        assert_eq!(
            replay(saved.clone(), "saved", &unrelated).err().unwrap().0,
            StatusCode::CONFLICT
        );
        assert!(replay(saved, "another-id", &same).is_err());
    }

    #[test]
    fn requests_and_shared_configuration_cannot_grant_media_write() {
        for forged in [
            json!({"kind":"export_voices","allow_media_write":true,"options":{"voice_export":{}}}),
            json!({"kind":"export_voices","options":{"allow_media_write":true,"voice_export":{}}}),
            json!({"kind":"export_voices","options":{"voice_export":{"allow_media_write":true}}}),
        ] {
            assert!(serde_json::from_value::<Submission>(forged).is_err());
        }
        assert!(
            serde_json::from_value::<SettingsInput>(json!({"allow_media_write":true})).is_err()
        );
    }
}
