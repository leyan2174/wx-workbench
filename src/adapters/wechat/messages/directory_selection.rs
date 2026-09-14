//! Explicit raw-directory wire validation and selection; not ordinary conversation identity.
use super::read::layout;
use crate::message::export::Target;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize)]
pub struct RawDirectoryTarget {
    #[serde(flatten)]
    pub target: Target,
    pub table_name: String,
    pub identity_status: String,
}

pub fn select_targets(
    targets: Vec<RawDirectoryTarget>,
    wanted: &BTreeSet<String>,
) -> Result<Vec<RawDirectoryTarget>> {
    let mut tables = BTreeSet::new();
    let mut usernames = BTreeSet::new();
    for entry in &targets {
        let hash = layout::canonical_table_hash(&entry.table_name).context("目录含非法消息表名")?;
        ensure!(
            tables.insert(entry.table_name.clone())
                && usernames.insert(entry.target.username.clone()),
            "消息表目录包含重复表或歧义 username"
        );
        match entry.identity_status.as_str() {
            "mapped" => ensure!(
                !entry.target.username.is_empty()
                    && layout::username_hash(&entry.target.username) == hash,
                "目录 username 与消息表不符"
            ),
            "unmapped" => ensure!(
                entry.target.username == format!("unknown_{hash}") && !entry.target.is_group,
                "未映射消息表身份无效"
            ),
            _ => anyhow::bail!("未知目录身份状态"),
        }
    }
    let mut selected = BTreeMap::new();
    let mut missing = Vec::new();
    for username in wanted {
        let table = layout::table_for_username(username);
        let entry = targets
            .iter()
            .find(|entry| entry.target.username == *username)
            .or_else(|| targets.iter().find(|entry| entry.table_name == table));
        let Some(entry) = entry else {
            missing.push(username.as_str());
            continue;
        };
        ensure!(
            entry.target.username == *username || entry.identity_status == "unmapped",
            "指定 username 与消息表已有映射冲突"
        );
        ensure!(
            selected
                .insert(entry.table_name.clone(), username.clone())
                .is_none(),
            "同一消息表被多个名称选择，请只保留一个精确 username"
        );
    }
    ensure!(
        missing.is_empty(),
        "以下 username 不在消息表目录中，未静默忽略：{}",
        missing.join(", ")
    );
    let mut result = Vec::new();
    for mut entry in targets {
        if !wanted.is_empty() {
            let Some(username) = selected.remove(&entry.table_name) else {
                continue;
            };
            if username != entry.target.username {
                // 用户给出的精确 username 的 MD5 已命中真实表；不按显示名猜测身份。
                entry.target.is_group = username.ends_with("@chatroom");
                entry.target.chat = username.clone();
                entry.target.username = username;
                entry.identity_status = "explicit_username".into();
            }
        }
        result.push(entry);
    }
    result.sort_by(|a, b| a.target.username.cmp(&b.target.username));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(username: &str, mapped: bool) -> RawDirectoryTarget {
        let hash = layout::username_hash(username);
        let target = if mapped {
            username.to_owned()
        } else {
            format!("unknown_{hash}")
        };
        RawDirectoryTarget {
            target: Target {
                username: target.clone(),
                chat: target,
                is_group: false,
            },
            table_name: layout::table_for_username(username),
            identity_status: if mapped { "mapped" } else { "unmapped" }.into(),
        }
    }

    #[test]
    fn legacy_validation_error_text_and_all_entry_validation_are_preserved() {
        let mut invalid = entry("bad", true);
        invalid.table_name = invalid.table_name.to_ascii_uppercase();
        let error = select_targets(
            vec![entry("wanted", true), invalid],
            &BTreeSet::from(["wanted".into()]),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "目录含非法消息表名");
        let error = select_targets(
            vec![entry("same", true), entry("same", true)],
            &BTreeSet::new(),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "消息表目录包含重复表或歧义 username");
        let mut invalid = entry("mapped", true);
        invalid.target.username = "other".into();
        assert_eq!(
            select_targets(vec![invalid], &BTreeSet::new())
                .unwrap_err()
                .to_string(),
            "目录 username 与消息表不符"
        );
        let mut invalid = entry("lost", false);
        invalid.target.is_group = true;
        assert_eq!(
            select_targets(vec![invalid], &BTreeSet::new())
                .unwrap_err()
                .to_string(),
            "未映射消息表身份无效"
        );
        let mut invalid = entry("known", true);
        invalid.identity_status = "other".into();
        assert_eq!(
            select_targets(vec![invalid], &BTreeSet::new())
                .unwrap_err()
                .to_string(),
            "未知目录身份状态"
        );
    }

    #[test]
    fn exact_unknown_selection_alias_rejection_and_sorting_are_preserved() {
        let placeholder = entry("lost", false).target.username;
        let selected = select_targets(
            vec![entry("lost", false)],
            &BTreeSet::from([placeholder.clone()]),
        )
        .unwrap();
        assert_eq!(selected[0].target.username, placeholder);
        assert_eq!(selected[0].identity_status, "unmapped");
        let error = select_targets(
            vec![entry("lost", false)],
            &BTreeSet::from(["lost".into(), placeholder]),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "同一消息表被多个名称选择，请只保留一个精确 username"
        );
        let error =
            select_targets(Vec::new(), &BTreeSet::from(["z".into(), "a".into()])).unwrap_err();
        assert_eq!(
            error.to_string(),
            "以下 username 不在消息表目录中，未静默忽略：a, z"
        );
        let selected =
            select_targets(vec![entry("z", true), entry("a", true)], &BTreeSet::new()).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|row| row.target.username.as_str())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
    }
}
