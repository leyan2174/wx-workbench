//! Contact wire projections. No database rows or storage-specific filtering here.
use crate::{
    adapters::wechat::contacts::SqliteContacts,
    business::contacts::{self, ContactQuery, ContactView},
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::path::Path;

pub fn contacts_from_path(path: &Path, query: Option<&str>, limit: usize) -> Result<Value> {
    project_from_path(path, query, limit, ContactView::VisibleDirectory)
}

pub(crate) fn project_from_path(
    path: &Path,
    query: Option<&str>,
    limit: usize,
    view: ContactView,
) -> Result<Value> {
    let source = match view {
        ContactView::People => SqliteContacts::new(path.into()),
        ContactView::VisibleDirectory => SqliteContacts::legacy_directory(path.into()),
    };
    let page = contacts::list(
        &source,
        ContactQuery {
            text: query,
            view,
            offset: 0,
            limit,
        },
    )?;
    project_page(page, view)
}

pub(crate) fn project_page(page: contacts::ContactPage, view: ContactView) -> Result<Value> {
    let projected: Vec<Value> = page.contacts.iter().map(|contact| match view {
        ContactView::People => json!({"username":contact.id.0, "display":contact.display()}),
        ContactView::VisibleDirectory => json!({
            "username":contact.id.0, "nick_name":contact.nickname.as_deref().unwrap_or(""),
            "remark":contact.remark.as_deref().unwrap_or(""), "alias":contact.alias.as_deref().unwrap_or(""),
            "description":contact.description.as_deref().unwrap_or(""), "phone":contact.phone.as_deref().unwrap_or(""),
            "display":contact.display(),
        }),
    }).collect();
    let mut size = 0usize;
    for item in &projected {
        size = size.saturating_add(serde_json::to_vec(item)?.len());
        ensure!(
            size <= 16 * 1024 * 1024,
            "contact result byte limit exceeded"
        );
    }
    Ok(json!({"contacts":projected, "total":page.total}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_and_legacy_projections_share_contact_identity_and_display() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("synthetic.db");
        rusqlite::Connection::open(&path).unwrap().execute_batch(
            "CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,local_type INTEGER,alias TEXT);
             INSERT INTO contact VALUES('b','Nick','Remark',0,1,'alias'),('a','Same','',0,1,NULL),('g@chatroom','Group','',0,1,NULL);"
        ).unwrap();
        let ordinary = project_from_path(&path, None, 10, ContactView::People).unwrap();
        let legacy = contacts_from_path(&path, None, 10).unwrap();
        assert_eq!(ordinary["total"], 2);
        assert_eq!(legacy["total"], 3);
        for person in ordinary["contacts"].as_array().unwrap() {
            let richer = legacy["contacts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["username"] == person["username"])
                .unwrap();
            assert_eq!(person["display"], richer["display"]);
        }
        assert_eq!(
            contacts_from_path(&path, Some("nick"), 10).unwrap()["total"],
            1
        );
        assert_eq!(
            project_from_path(&path, Some("nick"), 10, ContactView::People).unwrap()["total"],
            0
        );
        let page = contacts_from_path(&path, None, 0).unwrap();
        assert_eq!(page["total"], 3);
        assert!(page["contacts"].as_array().unwrap().is_empty());
    }
}
