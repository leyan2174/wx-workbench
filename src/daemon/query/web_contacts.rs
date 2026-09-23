//! Web contact and bound-account projections using the shared contact source.
use super::mcp_contacts::source;
use crate::daemon::cache::DbCache;
use crate::{adapters::wechat::contacts as wechat, business::contacts as domain};
use anyhow::{Context, Result};
use std::collections::HashMap;

pub async fn q_account_profile(
    db: &DbCache,
    runtime: &crate::runtime::RuntimeContext,
) -> Result<serde_json::Value> {
    let account = runtime
        .config
        .db_dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .context("account directory unavailable")?
        .to_owned();
    let source = source(db, &HashMap::new()).await?;
    tokio::task::spawn_blocking(move || account_profile(&source, &account)).await?
}

pub async fn q_web_contacts(
    db: &DbCache,
    request: crate::ipc::ContactsRequest,
) -> Result<serde_json::Value> {
    let source = source(db, &HashMap::new()).await?;
    tokio::task::spawn_blocking(move || {
        let page = domain::list(
            &source,
            domain::ContactQuery {
                text: request.query.as_deref(),
                offset: 0,
                limit: request.limit,
            },
        )?;
        super::contact_rows::project_web_page(page)
    })
    .await?
}

fn account_profile(source: &wechat::SqliteContacts, account: &str) -> Result<serde_json::Value> {
    use domain::ContactSource;
    let directory = source.contacts()?;
    let names = directory
        .contacts
        .iter()
        .map(|c| (c.id.0.clone(), String::new()))
        .collect();
    let username = crate::message::identity::self_username(account, &names);
    let contact = directory
        .contacts
        .iter()
        .find(|c| !username.is_empty() && c.id.0 == username)
        .context("current account profile unavailable")?;
    Ok(serde_json::json!({
        "nickname": contact.nickname,
        "username": contact.alias.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&username),
        "status": "已读取"
    }))
}

#[cfg(test)]
mod account_profile_tests {
    use super::*;

    #[test]
    fn web_contacts_preserve_real_nickname_separately_from_remark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contact.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE contact(username TEXT, nick_name TEXT, remark TEXT, verify_flag INTEGER);
            INSERT INTO contact VALUES ('wxid_friend', '微信昵称', '我的备注', 0);",
        )
        .unwrap();
        let source = wechat::SqliteContacts::new(path);
        let page = domain::list(
            &source,
            domain::ContactQuery {
                text: None,
                offset: 0,
                limit: 10,
            },
        )
        .unwrap();
        let result = super::super::contact_rows::project_web_page(page).unwrap();
        assert_eq!(result["contacts"][0]["nickname"], "微信昵称");
        assert_eq!(result["contacts"][0]["remark"], "我的备注");
        assert_eq!(result["contacts"][0]["display"], "我的备注");
        let page = domain::list(
            &source,
            domain::ContactQuery {
                text: None,
                offset: 0,
                limit: 10,
            },
        )
        .unwrap();
        let compact = super::super::contact_rows::project_page(page).unwrap();
        assert!(compact["contacts"][0].get("nickname").is_none());
    }

    #[test]
    fn profile_uses_bound_account_nickname_and_alias_not_remark_or_other_contact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contact.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE contact(username TEXT, nick_name TEXT, remark TEXT, alias TEXT);
            INSERT INTO contact VALUES ('wxid_me', '我的昵称', '备注不应显示', 'my_wechat');
            INSERT INTO contact VALUES ('wxid_other', '其他账号', '', 'other_wechat');",
        )
        .unwrap();
        let source = wechat::SqliteContacts::new(path);
        let profile = account_profile(&source, "wxid_me_ab12").unwrap();
        assert_eq!(profile["nickname"], "我的昵称");
        assert_eq!(profile["username"], "my_wechat");
        assert!(account_profile(&source, "wxid_missing_ab12").is_err());
        conn.execute(
            "UPDATE contact SET alias = '' WHERE username = 'wxid_me'",
            [],
        )
        .unwrap();
        assert_eq!(
            account_profile(&source, "wxid_me_ab12").unwrap()["username"],
            "wxid_me"
        );
    }
}
