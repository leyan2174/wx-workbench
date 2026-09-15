use super::*;
use serde_json::json;

#[test]
fn classifies_legacy_markers_without_reading_message_bodies() {
    for (data, expected) in [
        (
            json!({"messages":[{"success":false,"error":"ordinary message text"}]}),
            BusinessOutcome::Success,
        ),
        (
            json!({"status":"partial","success":false}),
            BusinessOutcome::Partial,
        ),
        (
            json!({"status":"refused","exit_code":1}),
            BusinessOutcome::Refused,
        ),
        (json!({"exit_code":2}), BusinessOutcome::Failure),
        (
            json!({"status":"error","exit_code":0}),
            BusinessOutcome::Failure,
        ),
        (json!({"ok":false}), BusinessOutcome::Failure),
        (json!({"success":false}), BusinessOutcome::Failure),
        (json!({"exit_code":3}), BusinessOutcome::Failure),
        (json!({"exit_code":"0"}), BusinessOutcome::Failure),
        (json!({"success":"true"}), BusinessOutcome::Failure),
        (
            json!({"error":"SYNTHETIC_PRIVATE_KEY"}),
            BusinessOutcome::Failure,
        ),
        (
            json!({"partial_legacy_compatibility":true}),
            BusinessOutcome::Success,
        ),
        (
            json!({"status":"ok","exit_code":0}),
            BusinessOutcome::Success,
        ),
    ] {
        assert_eq!(BusinessOutcome::from_legacy(&data), expected, "{data}");
    }
}

#[test]
fn checked_failure_is_typed_and_sanitized() {
    let failure = BusinessOutcome::from_legacy(&json!({"error":"SYNTHETIC_PRIVATE_KEY"}))
        .require_success()
        .unwrap_err();
    let error = anyhow::Error::new(failure).context("public context");
    assert_eq!(
        error.downcast_ref::<BusinessFailure>(),
        Some(&BusinessFailure(BusinessOutcome::Failure, None, None))
    );
    assert!(!format!("{error:#} {error:?}").contains("SYNTHETIC_PRIVATE_KEY"));
}

#[test]
fn aggregation_and_worker_exit_roundtrip() {
    assert_eq!(BusinessOutcome::from_counts(0, 0), BusinessOutcome::Success);
    assert_eq!(BusinessOutcome::from_counts(1, 1), BusinessOutcome::Partial);
    assert_eq!(BusinessOutcome::from_counts(0, 1), BusinessOutcome::Failure);
    for outcome in [
        BusinessOutcome::Success,
        BusinessOutcome::Partial,
        BusinessOutcome::Refused,
        BusinessOutcome::Failure,
    ] {
        assert_eq!(
            BusinessOutcome::from_worker_exit(outcome.worker_exit_code()),
            outcome
        );
    }
    assert_eq!(
        BusinessOutcome::from_worker_exit(10),
        BusinessOutcome::Failure
    );
    assert_eq!(
        BusinessOutcome::from_worker_exit(3),
        BusinessOutcome::Failure
    );
}

#[test]
fn response_classification_preserves_legacy_wire_schema() {
    use super::super::Response;
    let response =
        Response::ok(json!({"status":"partial","count":2,"message":"SYNTHETIC_PRIVATE_KEY"}));
    let before = serde_json::to_value(&response).unwrap();
    let failure = response.require_success().unwrap_err();
    assert_eq!(failure.0, BusinessOutcome::Partial);
    assert!(!format!("{failure:?} {failure}").contains("SYNTHETIC_PRIVATE_KEY"));
    assert_eq!(serde_json::to_value(&response).unwrap(), before);
    assert_eq!(
        before,
        json!({"ok":true,"status":"partial","count":2,"message":"SYNTHETIC_PRIVATE_KEY"})
    );
    assert_eq!(
        Response::err("SYNTHETIC_PRIVATE_KEY").outcome(),
        BusinessOutcome::Failure
    );
    assert_eq!(
        Response::ok(json!({"exit_code":2}))
            .require_success()
            .unwrap_err()
            .legacy_exit_code(),
        Some(2)
    );
}

#[test]
fn key_diagnostics_only_accept_whitelisted_codes_not_backend_text() {
    use super::super::Response;
    for diagnostic in [
        KeyStoreDiagnostic::Missing,
        KeyStoreDiagnostic::LegacyMigrationRequired,
        KeyStoreDiagnostic::Invalid,
        KeyStoreDiagnostic::WrongAccount,
        KeyStoreDiagnostic::Protection,
        KeyStoreDiagnostic::Conflict,
        KeyStoreDiagnostic::Busy,
        KeyStoreDiagnostic::Io,
    ] {
        let mut response = Response::err("SYNTHETIC_PRIVATE_KEY");
        response.data = json!({"error_code":diagnostic.code()});
        let failure = response.require_success().unwrap_err();
        assert_eq!(failure.0, diagnostic.outcome());
        assert_eq!(failure.public_message(), diagnostic.message());
        assert_eq!(
            BusinessFailure::from_service_code(failure.service_code()),
            Some(failure)
        );
        assert!(!format!("{failure:?} {failure}").contains("SYNTHETIC_PRIVATE_KEY"));
    }
    assert!(BusinessFailure::from_service_code("SYNTHETIC_PRIVATE_KEY").is_none());
}
#[test]
fn query_budget_diagnostics_have_distinct_private_wire_contracts() {
    use super::QueryLimitExceeded;
    let response = QueryLimitExceeded::ResponseLimitExceeded {
        operation: "history".into(),
        response_limit_bytes: 32 * 1024 * 1024,
    };
    assert_eq!(
        serde_json::to_value(&response).unwrap(),
        serde_json::json!({
            "code": "response_limit_exceeded", "operation": "history",
            "response_limit_bytes": 33554432
        })
    );
    let read = QueryLimitExceeded::QueryReadLimitExceeded {
        operation: "history".into(),
    };
    assert_eq!(
        serde_json::to_value(&read).unwrap(),
        serde_json::json!({
            "code": "query_read_limit_exceeded", "operation": "history"
        })
    );
    for error in [response, read] {
        let text = error.to_string();
        assert!(text.contains("--offset") && text.contains("--limit"));
        assert!(!text.contains("Business operation failed"));
    }
}
