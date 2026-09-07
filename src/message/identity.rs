//! 导出发送者语义：账号身份必须由目录候选与联系人用户名共同确认。
use std::collections::HashMap;

pub fn self_username(account_dir: &str, names: &HashMap<String, String>) -> String {
    if let Some((prefix, suffix)) = account_dir.rsplit_once('_') {
        if !prefix.is_empty()
            && suffix.len() >= 4
            && suffix.bytes().all(|b| b.is_ascii_hexdigit())
            && names.contains_key(prefix)
        {
            return prefix.to_owned();
        }
    }
    if names.contains_key(account_dir) {
        account_dir.to_owned()
    } else {
        String::new()
    }
}

pub fn export_sender(
    mapped: &str,
    prefix: &str,
    is_group: bool,
    chat: &str,
    display: &str,
    me: &str,
    names: &HashMap<String, String>,
) -> String {
    let username = if is_group {
        if !mapped.is_empty() && mapped != chat {
            mapped
        } else {
            prefix
        }
    } else {
        // 与旧导出一致：一对一对方名称优先，不把“给自己发消息”擅自重标。
        if mapped == chat {
            return display.to_owned();
        }
        mapped
    };
    if username.is_empty() {
        String::new()
    } else if username == me {
        "me".into()
    } else {
        names
            .get(username)
            .cloned()
            .unwrap_or_else(|| username.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_legacy_sender_combinations() {
        let names = HashMap::from([
            ("self".into(), "我的昵称".into()),
            ("peer".into(), "联系人".into()),
        ]);
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/identity-golden.json"))
                .unwrap();
        for case in cases.as_array().unwrap() {
            let actual = export_sender(
                case["mapped"].as_str().unwrap(),
                case["prefix"].as_str().unwrap(),
                case["group"].as_bool().unwrap(),
                case["chat"].as_str().unwrap(),
                "聊天备注",
                "self",
                &names,
            );
            assert_eq!(actual, case["expected"].as_str().unwrap(), "{case}");
        }
    }

    #[test]
    fn confirms_self_by_username_not_display_name() {
        let names = HashMap::from([
            ("wxid_me".into(), "我的昵称".into()),
            ("wxid_me_abcd".into(), "另一个".into()),
        ]);
        assert_eq!(self_username("wxid_me_abcd", &names), "wxid_me");
        assert_eq!(self_username("wxid_me", &names), "wxid_me");
        assert_eq!(self_username("我的昵称", &names), "");
        assert_eq!(self_username("wxid_me_xyz1", &names), "");
        assert_eq!(self_username("wxid_me_abc", &names), "");
    }

    #[test]
    fn preserves_sender_priority_and_unknown_identity() {
        let names = HashMap::from([
            ("self".into(), "我的昵称".into()),
            ("peer".into(), "联系人".into()),
        ]);
        assert_eq!(
            export_sender("self", "peer", true, "room@chatroom", "群", "self", &names),
            "me"
        );
        assert_eq!(
            export_sender(
                "room@chatroom",
                "self",
                true,
                "room@chatroom",
                "群",
                "self",
                &names
            ),
            "me"
        );
        assert_eq!(
            export_sender("", "self", false, "peer", "备注", "self", &names),
            ""
        );
        assert_eq!(
            export_sender("peer", "", false, "peer", "备注", "self", &names),
            "备注"
        );
        assert_eq!(
            export_sender(
                "unknown",
                "self",
                true,
                "room@chatroom",
                "群",
                "self",
                &names
            ),
            "unknown"
        );
        assert_eq!(
            export_sender("self", "", false, "self", "我的昵称", "self", &names),
            "我的昵称"
        );
    }
}
