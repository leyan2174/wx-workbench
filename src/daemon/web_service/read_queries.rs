//! Explicit Web read-query allowlist. No generic IPC request is deserialized.
use crate::{ipc::Request, service::web::Call};
use anyhow::Result;

pub(super) fn request(call: Call) -> Result<Request> {
    call.validate_read()?;
    Ok(match call {
        Call::History {
            chat,
            limit,
            offset,
            since,
            until,
            msg_type,
            msg_types,
            oldest_first,
            with_meta,
            debug_source,
        } => Request::History {
            chat,
            limit,
            offset,
            since,
            until,
            msg_type,
            msg_types,
            oldest_first,
            with_meta,
            debug_source,
        },
        Call::Search {
            keyword,
            chats,
            limit,
            since,
            until,
            msg_type,
            with_meta,
            debug_source,
        } => Request::Search {
            keyword,
            chats,
            limit,
            since,
            until,
            msg_type,
            with_meta,
            debug_source,
        },
        Call::Unread {
            limit,
            filter,
            with_meta,
            debug_source,
        } => Request::Unread {
            limit,
            filter,
            with_meta,
            debug_source,
        },
        Call::Members { chat } => Request::Members { chat },
        Call::Stats {
            chat,
            since,
            until,
            with_meta,
            debug_source,
        } => Request::Stats {
            chat,
            since,
            until,
            with_meta,
            debug_source,
        },
        Call::Favorites {
            limit,
            fav_type,
            query,
        } => Request::Favorites {
            limit,
            fav_type,
            query,
        },
        Call::BizArticles {
            limit,
            account,
            since,
            until,
            unread,
        } => Request::BizArticles {
            limit,
            account,
            since,
            until,
            unread,
        },
        Call::SnsFeed {
            limit,
            since,
            until,
            user,
        } => Request::SnsFeed {
            limit,
            since,
            until,
            user,
        },
        Call::SnsSearch {
            keyword,
            limit,
            since,
            until,
            user,
        } => Request::SnsSearch {
            keyword,
            limit,
            since,
            until,
            user,
        },
        Call::SnsNotifications {
            limit,
            since,
            until,
            include_read,
        } => Request::SnsNotifications {
            limit,
            since,
            until,
            include_read,
        },
        Call::VoiceMessages {
            chat,
            limit,
            offset,
            since,
            until,
        } => Request::VoiceMessages {
            chat,
            limit,
            offset,
            since,
            until,
        },
        Call::DecodeTransfer {
            chat,
            local_id,
            create_time,
        } => Request::DecodeTransfer {
            chat,
            local_id,
            create_time,
        },
        Call::DecodeLocation {
            chat,
            local_id,
            create_time,
        } => Request::DecodeLocation {
            chat,
            local_id,
            create_time,
        },
        Call::DecodeRefer {
            chat,
            local_id,
            create_time,
        } => Request::DecodeRefer {
            chat,
            local_id,
            create_time,
        },
        Call::DecodeFileMessage {
            chat,
            local_id,
            create_time,
        } => Request::DecodeFileMessage {
            chat,
            local_id,
            create_time,
        },
        Call::DecodeRecordItem {
            chat,
            local_id,
            create_time,
            item_index,
        } => Request::DecodeRecordItem {
            chat,
            local_id,
            create_time,
            item_index,
        },
        _ => anyhow::bail!("not a read query"),
    })
}
