//! Strict reply selection over a live message snapshot; physical coordinates are projection only.
use crate::adapters::wechat::messages::{reply, RawMessage, Snapshot};
use crate::business::messages::{Conversation, Error};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{collections::HashMap, path::Path};

const MAX_DECODED_BYTES: usize = 131_072;

#[derive(Serialize)]
pub struct LegacySource {
    username: String,
    local_id: i64,
    create_time: i64,
    source: String,
}

pub enum Outcome {
    NotReply,
    InvalidContent,
    Found {
        parsed: Box<reply::ParsedReply>,
        source: LegacySource,
    },
}

pub fn decode(
    snapshot: &Snapshot,
    raw: &RawMessage,
    database: &Path,
    display: &str,
    names: &HashMap<String, String>,
) -> Result<Outcome> {
    let Conversation::Known(username) = snapshot.conversation(&raw.reference)? else {
        anyhow::bail!(Error::InvalidData);
    };
    let local_id = raw.local_id.context("message local identity unavailable")?;
    let base = if raw.local_type > u32::MAX as i64 {
        raw.local_type & 0xffff_ffff
    } else {
        raw.local_type
    };
    if base != 49 {
        return Ok(Outcome::NotReply);
    }
    let account = database
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let me = crate::message::identity::self_username(account, names);
    let parsed = (|| -> Result<_> {
        let bytes = raw.detached_content().bounded_decode(MAX_DECODED_BYTES)?;
        let text = String::from_utf8_lossy(&bytes);
        let body = if username.ends_with("@chatroom") {
            crate::message::split_group_content(&text).1
        } else {
            &text
        };
        reply::parse_refer(body, username, display, &me, names)
    })();
    Ok(match parsed {
        Ok(parsed) => Outcome::Found {
            parsed: Box::new(parsed),
            source: LegacySource {
                username: username.clone(),
                local_id,
                create_time: raw.timestamp,
                source: raw.logical_source.clone(),
            },
        },
        // Keep parser details, raw XML and compressed data out of the public error.
        Err(_) => Outcome::InvalidContent,
    })
}
