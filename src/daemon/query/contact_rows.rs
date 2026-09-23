//! One contact wire projection; selection and ordering belong to the business layer.
use crate::business::contacts::ContactPage;
use anyhow::{ensure, Result};
use serde_json::{json, Value};

pub(crate) fn project_page(page: ContactPage) -> Result<Value> {
    project(page, false)
}

pub(crate) fn project_web_page(page: ContactPage) -> Result<Value> {
    project(page, true)
}

fn project(page: ContactPage, include_names: bool) -> Result<Value> {
    let mut projected = Vec::with_capacity(page.contacts.len());
    let mut size = 0usize;
    for contact in &page.contacts {
        let mut item = json!({"username":contact.id.0, "display":contact.display()});
        if include_names {
            item["nickname"] = json!(contact.nickname);
            item["remark"] = json!(contact.remark);
        }
        size = size.saturating_add(serde_json::to_vec(&item)?.len());
        ensure!(
            size <= 16 * 1024 * 1024,
            "contact result byte limit exceeded"
        );
        projected.push(item);
    }
    Ok(json!({"contacts":projected, "total":page.total}))
}
