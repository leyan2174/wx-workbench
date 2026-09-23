//! Tag response projection; storage and selection live below the host.
use crate::daemon::cache::DbCache;
use crate::{adapters::wechat::contacts as wechat, business::contacts as domain};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use wechat::{
    MAX_ASSOCIATIONS, MAX_BUFFER_BYTES, MAX_LABELS, MAX_RESULT_TEXT_BYTES, MAX_TEXT_BYTES,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagMember {
    pub username: String,
    pub display_name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactTag {
    pub name: String,
    pub member_count: usize,
    pub members: Vec<TagMember>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactTags {
    pub total_tags: usize,
    pub total_associations: usize,
    pub tags: Vec<ContactTag>,
}

fn project(tag: domain::Tag) -> ContactTag {
    ContactTag {
        name: tag.name,
        member_count: tag.members.len(),
        members: tag
            .members
            .into_iter()
            .map(|member| TagMember {
                username: member.id.0,
                display_name: member.display_name,
            })
            .collect(),
    }
}
pub(super) async fn source(
    db: &DbCache,
    names: &HashMap<String, String>,
) -> Result<wechat::SqliteContacts> {
    let [primary, compatibility] = wechat::source_keys();
    let path = match db.get(primary).await? {
        Some(path) => path,
        None => db
            .get(compatibility)
            .await?
            .context("contact database unavailable")?,
    };
    let mut source = wechat::SqliteContacts::new(path);
    source.display_names = names.clone();
    Ok(source)
}
pub async fn q_contact_tags(db: &DbCache, names: &HashMap<String, String>) -> Result<ContactTags> {
    let source = source(db, names).await?;
    tokio::task::spawn_blocking(move || {
        use domain::ContactSource;
        source.tags().map(project_tags).map_err(anyhow::Error::from)
    })
    .await?
}
pub async fn q_tag_members(
    db: &DbCache,
    names: &HashMap<String, String>,
    tag_name: &str,
) -> Result<ContactTag> {
    validate_query(tag_name)?;
    let source = source(db, names).await?;
    let tag_name = tag_name.to_owned();
    tokio::task::spawn_blocking(move || {
        domain::tag(&source, &tag_name)
            .map(project)
            .map_err(tag_error)
    })
    .await?
}
fn validate_query(query: &str) -> Result<()> {
    domain::validate_tag_query(query).map_err(|error| match error {
        domain::Error::Limit => anyhow::anyhow!("tag query byte limit exceeded"),
        other => other.into(),
    })
}
#[cfg(test)]
pub fn select_tag<'a>(tags: &'a ContactTags, query: &str) -> Result<&'a ContactTag> {
    validate_query(query)?;
    let index = domain::select_name(tags.tags.iter().map(|tag| tag.name.as_str()), query)
        .map_err(tag_error)?;
    Ok(&tags.tags[index])
}
fn tag_error(error: domain::Error) -> anyhow::Error {
    match error {
        domain::Error::Ambiguous => anyhow::Error::new(error).context("ambiguous tag name"),
        domain::Error::NotFound => anyhow::anyhow!("tag not found"),
        other => other.into(),
    }
}
#[cfg(test)]
pub fn contact_tags_from_path(path: &Path, names: &HashMap<String, String>) -> Result<ContactTags> {
    Ok(project_tags(wechat::read_tags(path, names)?))
}
fn project_tags(tags: Vec<domain::Tag>) -> ContactTags {
    let tags: Vec<_> = tags.into_iter().map(project).collect();
    ContactTags {
        total_tags: tags.len(),
        total_associations: tags.iter().map(|tag| tag.member_count).sum(),
        tags,
    }
}

#[cfg(test)]
#[path = "../../../tests/fixtures/mcp-contacts/tests.rs"]
mod tests;
