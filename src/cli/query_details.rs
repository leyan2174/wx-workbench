use super::output::{print_value, resolve};
use crate::ipc::Request;
use crate::service::{query_client, time};
use anyhow::{ensure, Result};
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum Command {
    /// List contact tags and membership counts (read-only)
    Tags {
        #[arg(long)]
        json: bool,
    },
    /// List members of one uniquely matched contact tag (read-only)
    TagMembers {
        #[arg(value_parser = target)]
        tag_name: String,
        #[arg(long)]
        json: bool,
    },
    /// Decode a quoted reply without reading or exporting its attachments
    DecodeRefer(MessageArgs),
    /// Inspect a file message and its existing local reference (read-only)
    DecodeFileMessage(MessageArgs),
    /// Inspect a zero-based item in a forwarded record (read-only)
    DecodeRecordItem {
        #[arg(value_parser = target)]
        chat: String,
        #[arg(value_parser = clap::value_parser!(i64).range(1..))]
        local_id: i64,
        #[arg(value_parser = clap::value_parser!(i64).range(0..))]
        item_index: i64,
        /// Unix seconds; zero means no timestamp filter
        #[arg(default_value_t = 0, allow_negative_numbers = true)]
        create_time: i64,
        #[arg(long)]
        json: bool,
    },
    /// Read-only voice metadata
    VoiceMessages {
        #[arg(value_parser = target)]
        chat: String,
        #[arg(short = 'n', long, default_value_t = 20, value_parser = limit)]
        limit: usize,
        #[arg(long, default_value_t = 0, value_parser = offset)]
        offset: usize,
        /// Local YYYY-MM-DD, YYYY-MM-DD HH:MM or YYYY-MM-DD HH:MM:SS
        #[arg(long, value_parser = since)]
        since: Option<i64>,
        /// Local date/time; a date alone includes the entire day
        #[arg(long, value_parser = until)]
        until: Option<i64>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
pub struct MessageArgs {
    #[arg(value_parser = target)]
    chat: String,
    #[arg(value_parser = clap::value_parser!(i64).range(1..))]
    local_id: i64,
    /// Unix seconds; zero means no timestamp filter
    #[arg(default_value_t = 0, allow_negative_numbers = true)]
    create_time: i64,
    #[arg(long)]
    json: bool,
}

fn target(raw: &str) -> std::result::Result<String, String> {
    if raw.trim().is_empty() || raw.chars().count() > 4096 {
        return Err("Expected a nonblank target of at most 4096 characters".into());
    }
    Ok(raw.to_owned())
}

fn limit(raw: &str) -> std::result::Result<usize, String> {
    raw.parse::<usize>()
        .ok()
        .filter(|n| (1..=500).contains(n))
        .ok_or_else(|| "limit must be between 1 and 500".into())
}

fn offset(raw: &str) -> std::result::Result<usize, String> {
    raw.parse::<usize>()
        .ok()
        .filter(|n| *n <= 1_000_000)
        .ok_or_else(|| "offset must be between 0 and 1000000".into())
}

fn since(raw: &str) -> std::result::Result<i64, String> {
    time::parse_time(raw).map_err(|error| error.to_string())
}

fn until(raw: &str) -> std::result::Result<i64, String> {
    time::parse_time_end(raw).map_err(|error| error.to_string())
}

impl Command {
    pub(super) fn json(&self) -> bool {
        match self {
            Self::Tags { json }
            | Self::TagMembers { json, .. }
            | Self::DecodeRecordItem { json, .. }
            | Self::VoiceMessages { json, .. } => *json,
            Self::DecodeRefer(args) | Self::DecodeFileMessage(args) => args.json,
        }
    }

    fn into_request(self) -> Result<Request> {
        Ok(match self {
            Self::Tags { .. } => Request::ContactTags,
            Self::TagMembers { tag_name, .. } => Request::TagMembers { tag_name },
            Self::DecodeRefer(args) => Request::DecodeRefer {
                chat: args.chat,
                local_id: args.local_id,
                create_time: args.create_time,
            },
            Self::DecodeFileMessage(args) => Request::DecodeFileMessage {
                chat: args.chat,
                local_id: args.local_id,
                create_time: args.create_time,
            },
            Self::DecodeRecordItem {
                chat,
                local_id,
                item_index,
                create_time,
                ..
            } => Request::DecodeRecordItem {
                chat,
                local_id,
                item_index,
                create_time,
            },
            Self::VoiceMessages {
                chat,
                limit,
                offset,
                since,
                until,
                ..
            } => {
                ensure!(
                    !matches!((since, until), (Some(start), Some(end)) if start > end),
                    "since exceeds until"
                );
                Request::VoiceMessages {
                    chat,
                    limit,
                    offset,
                    since,
                    until,
                }
            }
        })
    }
}

pub(super) fn cmd(command: Command) -> Result<()> {
    let json = command.json();
    // The query client rejects transport and business failures before output.
    let response = query_client::send(command.into_request()?)?;
    print_value(&response.data, &resolve(json))
}

#[cfg(test)]
#[path = "query_details_tests.rs"]
mod tests;
