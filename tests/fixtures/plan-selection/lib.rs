#[path = "../../../src/toolkit/chat_plan_selection.rs"]
pub mod selection;

#[cfg(test)]
mod tests {
    use super::selection::{Mode, Plan};
    use serde_json::{json, Value};
    use std::io::Read;

    #[test]
    fn exact_legacy_csv_modes_identity_order_and_users_semantics() {
        let oracle: Value = serde_json::from_str(include_str!("oracle.json")).unwrap();
        assert_eq!(oracle["fields"].as_array().unwrap().len(), 12);
        for case in oracle["cases"].as_array().unwrap() {
            let mode = if case["mode"] == "blacklist" {
                Mode::Blacklist
            } else {
                Mode::Whitelist
            };
            let result =
                Plan::read(case["csv"].as_str().unwrap().as_bytes(), mode).and_then(|plan| {
                    plan.select(
                        case["valid"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_str().unwrap()),
                    )
                    .map(|v| v.to_vec())
                });
            if case["error"] == true {
                assert!(result.is_err(), "{case}");
            } else {
                assert_eq!(json!(result.unwrap()), case["expected"], "{case}");
            }
        }
        assert_eq!(oracle["cases"].as_array().unwrap().len(), 56);
    }

    #[test]
    fn malformed_csv_never_silently_selects_everyone() {
        for bytes in [
            b"".as_slice(),
            b"username,export,export\npeer,0,1\n",
            b"username,username\npeer,other\n",
            b"username,export\npeer\n",
            b"username,export\npeer,1,extra\n",
            b"username,export\n\"peer,1\n",
            b"username,export\n\"peer\"junk,1\n",
            b"username,export\npe\"er,1\n",
            b"username,export\n\xff,1\n",
            b"username,export\npe\0er,1\n",
        ] {
            assert!(Plan::read(bytes, Mode::Blacklist).is_err(), "{bytes:?}");
        }
    }

    #[test]
    fn duplicated_account_targets_and_limits_are_rejected() {
        let plan = Plan::read(b"username,export\npeer,1\n".as_slice(), Mode::Whitelist).unwrap();
        assert!(plan.select(["peer", "peer"]).is_err());
        assert!(Plan::read(
            std::io::repeat(b'x').take(16 * 1024 * 1024 + 1),
            Mode::Blacklist
        )
        .is_err());
        let mut rows = String::from("username,export\n");
        for id in 0..100_001 {
            rows.push_str(&format!("u{id},0\n"));
        }
        assert!(Plan::read(rows.as_bytes(), Mode::Blacklist).is_err());
    }
}
