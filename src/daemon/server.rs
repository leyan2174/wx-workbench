use anyhow::Result;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::cache::DbCache;
use super::query::Names;
use super::query_state::QueryState;
use crate::ipc::{Request, Response};

#[cfg(test)]
#[path = "server_contacts_tests.rs"]
mod contacts_tests;

#[cfg(test)]
#[path = "server_chat_tests.rs"]
mod chat_tests;

/// 启动 IPC server（Windows named pipe）
pub async fn serve(
    state: Arc<QueryState>,
    pipe_name: &str,
) -> Result<()> {
    #[cfg(windows)]
    serve_windows(state, pipe_name).await?;
    Ok(())
}

#[cfg(windows)]
async fn serve_windows(
    state: Arc<QueryState>,
    pipe_name: &str,
) -> Result<()> {
    use interprocess::local_socket::{tokio::prelude::*, GenericNamespaced, ListenerOptions};

    // 库自动补上 Windows 管道前缀；名称统一来自启动时固定的账号上下文。
    let name = pipe_name.to_ns_name::<GenericNamespaced>()?;
    let opts = ListenerOptions::new().name(name);
    let listener = opts.create_tokio()?;
    let connections = Arc::new(tokio::sync::Semaphore::new(64));

    eprintln!("[server] 监听账号管道 {pipe_name}");

    loop {
        // Backpressure the listener instead of spawning unbounded waiting handlers.
        let permit = Arc::clone(&connections).acquire_owned().await?;
        let conn = listener.accept().await?;
        let state = Arc::clone(&state);

        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = handle_connection_windows(conn, state).await {
                eprintln!("[server] 连接处理错误: {}", e);
            }
        });
    }
}

#[cfg(windows)]
async fn handle_connection_windows(
    conn: interprocess::local_socket::tokio::Stream,
    state: Arc<QueryState>,
) -> Result<()> {
    let (reader, mut writer) = tokio::io::split(conn);
    let line = match read_initial_request_frame(&mut BufReader::new(reader)).await {
        Ok(Some(line)) => line,
        Ok(None) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => return Ok(()),
        Err(_) => {
            let resp = Response::err("Invalid request frame (maximum 64 KiB)");
            writer.write_all(resp.to_json_line()?.as_bytes()).await?;
            return Ok(());
        }
    };

    let req: Request = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => {
            let resp = Response::err(format!("JSON 解析错误: {}", e));
            writer.write_all(resp.to_json_line()?.as_bytes()).await?;
            return Ok(());
        }
    };

    let resp = dispatch_state(req, &state).await;
    writer.write_all(resp.to_json_line()?.as_bytes()).await?;
    Ok(())
}

pub(super) async fn dispatch_state(req: Request, state: &QueryState) -> Response {
    if matches!(req, Request::Ping) {
        return Response::ok(serde_json::json!({ "pong": true }));
    }
    match state.snapshot().await {
        Ok(lease) => {
            let response = dispatch(req, lease.db(), lease.names()).await;
            drop(lease);
            response
        }
        Err(_) => Response::err(
            "Query initialization failed; check account configuration and keys, then retry",
        ),
    }
}

pub(super) const MAX_REQUEST_FRAME_BYTES: usize = 64 * 1024;

pub(super) async fn read_initial_request_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    tokio::time::timeout(std::time::Duration::from_secs(5), read_request_frame(reader))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Initial request frame timed out")
        })?
}

pub(super) async fn read_request_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if frame.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |position| position + 1);
        if count > MAX_REQUEST_FRAME_BYTES - frame.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Request frame exceeds 64 KiB",
            ));
        }
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            frame.pop();
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            break;
        }
    }
    String::from_utf8(frame)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

async fn dispatch(req: Request, db: &DbCache, names: &tokio::sync::RwLock<Arc<Names>>) -> Response {
    use super::query;
    use crate::ipc::Request::*;

    // 取 guard → O(1) clone Arc → 立即 drop 锁。后续 await 期间不持有锁，
    // 多个并发 IPC 请求可以真正并行。Names 本身不可变（由 daemon 启动时
    // 一次性构建），共享 Arc 即可。
    let names_arc: Arc<Names> = {
        let guard = names.read().await;
        Arc::clone(&*guard)
    };

    match req {
        Ping => Response::ok(serde_json::json!({ "pong": true })),
        LatencyProbe { limit } => match db.latency_probe(limit).await {
            Ok(probe) => match serde_json::to_value(probe) {
                Ok(value) => Response::ok(value),
                Err(error) => Response::err(error.to_string()),
            },
            Err(error) => Response::err(error.to_string()),
        },
        ResolveChat { chat } => match query::mcp_voice::resolve_exact_chat(&chat, &names_arc.map) {
            Ok(username) => Response::ok(serde_json::json!({"username": username})),
            Err(_) => Response::err("Chat has no unique exact match"),
        },
        ContactTags => match query::mcp_contacts::q_contact_tags(db, &names_arc.map).await {
            Ok(value) => Response::ok(serde_json::json!({
                "total_tags":value.total_tags,
                "total_associations":value.total_associations,
                "tags":value.tags.into_iter().map(|tag| serde_json::json!({
                    "name":tag.name,"member_count":tag.member_count
                })).collect::<Vec<_>>()
            })),
            Err(error) => Response::err(error.to_string()),
        },
        TagMembers { tag_name } => {
            match query::mcp_contacts::q_tag_members(db, &names_arc.map, &tag_name).await {
                Ok(value) => Response::ok(serde_json::json!(value)),
                Err(error) => Response::err(error.to_string()),
            }
        }
        DecodeRefer {
            chat,
            local_id,
            create_time,
        } => match query::q_decode_refer(db, &names_arc, &chat, local_id, create_time).await {
            Ok(value) => Response::ok(value),
            Err(error) => Response::err(error.to_string()),
        },
        DecodeFileMessage {
            chat,
            local_id,
            create_time,
        } => {
            match query::mcp_attachments::q_attachment_reference(
                db,
                &names_arc,
                &chat,
                local_id,
                create_time,
                None,
            )
            .await
            {
                Ok(value) => Response::ok(value),
                Err(error) => Response::err(error.to_string()),
            }
        }
        DecodeRecordItem {
            chat,
            local_id,
            item_index,
            create_time,
        } => {
            match query::mcp_attachments::q_attachment_reference(
                db,
                &names_arc,
                &chat,
                local_id,
                create_time,
                Some(item_index),
            )
            .await
            {
                Ok(value) => Response::ok(value),
                Err(error) => Response::err(error.to_string()),
            }
        }
        DecodeVoice { chat, local_id } | TranscribeVoice { chat, local_id } => {
            let limits = crate::toolkit::asr::prepared_audio::Limits {
                max_audio_bytes: crate::toolkit::asr::database_media::MAX_VOICE_BYTES,
                // 为外层 prepared_audio 和 IPC 包装预留空间，不能靠放宽 MCP 帧解决。
                max_response_bytes: crate::ipc::MAX_PREPARED_VOICE_RESPONSE_BYTES - 1024,
            };
            match query::mcp_audio::q_prepare_voice(db, &names_arc, &chat, local_id, limits).await {
                Ok(value) => Response::ok(value),
                Err(_) => Response::ok(serde_json::json!({
                    "exit_code": 1,
                    "status": "error",
                    "message": "Voice preparation failed"
                })),
            }
        }
        DecodeImage {
            chat,
            local_id,
            create_time,
            output_root,
            image_key_file,
        } => {
            match query::mcp_image::q_decode_image_with_key_file(
                db,
                &names_arc,
                &chat,
                local_id,
                create_time,
                std::path::Path::new(&output_root),
                image_key_file.as_deref().map(std::path::Path::new),
            )
            .await
            {
                Ok(value) => Response::ok(value),
                // 解码拒绝属于业务失败；保持传输可用，且不泄露路径或密钥。
                Err(_) => Response::ok(serde_json::json!({
                    "exit_code": 1,
                    "status": "error",
                    "message": "Image export failed"
                })),
            }
        }
        VoiceMessages {
            chat,
            limit,
            offset,
            since,
            until,
        } => {
            let result = async {
                anyhow::ensure!(
                    (1..=500).contains(&limit) && offset <= 1_000_000,
                    "语音查询分页超出范围"
                );
                let username = query::mcp_voice::resolve_exact_chat(&chat, &names_arc.map)?;
                let rows = query::mcp_voice::q_voice_messages(
                    db,
                    &query::mcp_voice::VoiceQuery {
                        username,
                        limit,
                        offset,
                        since,
                        until,
                    },
                )
                .await?;
                Ok::<_, anyhow::Error>(serde_json::json!({"voices": rows, "count": rows.len()}))
            }
            .await;
            match result {
                Ok(value) => Response::ok(value),
                Err(error) => Response::err(error.to_string()),
            }
        }
        ExportChatList => match query::q_export_chat_list(db, &names_arc).await {
            Ok(value) => Response::ok(value),
            Err(error) => Response::err(error.to_string()),
        },
        ExportDirectoryCatalog => match query::q_export_directory_catalog(db, &names_arc).await {
            Ok(value) => Response::ok(value),
            Err(error) => Response::err(error.to_string()),
        },
        ExportDelta {
            username,
            start,
            end,
        } => {
            match query::q_export_delta_username(db, &names_arc, username, Some(start), end).await {
                Ok(value) => Response::ok(value),
                Err(error) => Response::err(error.to_string()),
            }
        }
        ExportChatByUsername { username } => {
            match query::q_export_username(db, &names_arc, username).await {
                Ok(value) => Response::ok(value),
                Err(error) => Response::err(error.to_string()),
            }
        }
        ExportDirectoryByUsername { username } => {
            match query::q_export_directory_by_username(db, &names_arc, username).await {
                Ok(value) => Response::ok(value),
                Err(error) => Response::err(error.to_string()),
            }
        }
        ExportChat { chat } => match query::q_export_chat(db, &names_arc, &chat).await {
            Ok(value) => Response::ok(value),
            Err(error) => Response::err(error.to_string()),
        },
        DecodeTransfer {
            chat,
            local_id,
            create_time,
        } => match query::q_decode(
            db,
            &names_arc,
            &chat,
            local_id,
            create_time,
            query::DecodeKind::Transfer,
        )
        .await
        {
            Ok(value) => Response::ok(value),
            Err(error) => Response::err(error.to_string()),
        },
        DecodeLocation {
            chat,
            local_id,
            create_time,
        } => match query::q_decode(
            db,
            &names_arc,
            &chat,
            local_id,
            create_time,
            query::DecodeKind::Location,
        )
        .await
        {
            Ok(value) => Response::ok(value),
            Err(error) => Response::err(error.to_string()),
        },
        Sessions {
            limit,
            with_meta,
            debug_source,
        } => match query::q_sessions(db, &names_arc, limit, with_meta, debug_source).await {
            Ok(v) => Response::ok(v),
            Err(e) => Response::err(e.to_string()),
        },
        History {
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
        } => {
            match query::q_history(
                db,
                &names_arc,
                &chat,
                limit,
                offset,
                since,
                until,
                msg_type,
                with_meta,
                debug_source,
                msg_types.as_deref(),
                oldest_first,
            )
            .await
            {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Search {
            keyword,
            chats,
            limit,
            since,
            until,
            msg_type,
            with_meta,
            debug_source,
        } => {
            match query::q_search(
                db,
                &names_arc,
                &keyword,
                chats,
                limit,
                since,
                until,
                msg_type,
                with_meta,
                debug_source,
            )
            .await
            {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Contacts {
            query,
            limit,
            legacy_view,
        } => {
            let result = if legacy_view {
                query::mcp_contacts_legacy::q_contacts_legacy(db, query.as_deref(), limit).await
            } else {
                query::q_contacts(&names_arc, query.as_deref(), limit).await
            };
            match result {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Unread {
            limit,
            filter,
            with_meta,
            debug_source,
        } => match query::q_unread(db, &names_arc, limit, filter, with_meta, debug_source).await {
            Ok(v) => Response::ok(v),
            Err(e) => Response::err(e.to_string()),
        },
        Members { chat } => match query::q_members(db, &names_arc, &chat).await {
            Ok(v) => Response::ok(v),
            Err(e) => Response::err(e.to_string()),
        },
        NewMessages {
            state,
            limit,
            with_meta,
            debug_source,
        } => {
            match query::q_new_messages(db, &names_arc, state, limit, with_meta, debug_source).await
            {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Favorites {
            limit,
            fav_type,
            query,
        } => match query::q_favorites(db, limit, fav_type, query).await {
            Ok(v) => Response::ok(v),
            Err(e) => Response::err(e.to_string()),
        },
        Stats {
            chat,
            since,
            until,
            with_meta,
            debug_source,
        } => {
            match query::q_stats(db, &names_arc, &chat, since, until, with_meta, debug_source).await
            {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        SnsNotifications {
            limit,
            since,
            until,
            include_read,
        } => {
            match query::q_sns_notifications(db, &names_arc, limit, since, until, include_read)
                .await
            {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        SnsFeed {
            limit,
            since,
            until,
            user,
        } => match query::q_sns_feed(db, &names_arc, limit, since, until, user.as_deref()).await {
            Ok(v) => Response::ok(v),
            Err(e) => Response::err(e.to_string()),
        },
        SnsSearch {
            keyword,
            limit,
            since,
            until,
            user,
        } => {
            match query::q_sns_search(
                db,
                &names_arc,
                &keyword,
                limit,
                since,
                until,
                user.as_deref(),
            )
            .await
            {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        ReloadConfig => {
            match query::load_names_with_retry(db, 3, std::time::Duration::from_millis(300)).await {
                Ok(mut fresh) => {
                    fresh.msg_db_keys = names_arc.msg_db_keys.clone();
                    fresh.biz_msg_db_keys = names_arc.biz_msg_db_keys.clone();
                    let count = fresh.map.len();
                    *names.write().await = Arc::new(fresh);
                    Response::ok(serde_json::json!({ "reloaded": true, "contacts": count }))
                }
                Err(error) => Response::err(format!("重新加载联系人失败: {}", error)),
            }
        }
        BizArticles {
            limit,
            account,
            since,
            until,
            unread,
        } => {
            match query::q_biz_articles(db, &names_arc, limit, account, since, until, unread).await
            {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Attachments {
            chat,
            image_metadata,
            kinds,
            limit,
            offset,
            since,
            until,
            with_meta,
            debug_source,
        } => {
            let result = if image_metadata {
                query::q_attachments_with_image_metadata(
                    db,
                    &names_arc,
                    &chat,
                    kinds,
                    limit,
                    offset,
                    since,
                    until,
                    with_meta,
                    debug_source,
                )
                .await
            } else {
                query::q_attachments(
                    db,
                    &names_arc,
                    &chat,
                    kinds,
                    limit,
                    offset,
                    since,
                    until,
                    with_meta,
                    debug_source,
                )
                .await
            };
            match result {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Extract {
            attachment_id,
            output,
            overwrite,
        } => match query::q_extract(db, &names_arc, &attachment_id, &output, overwrite).await {
            Ok(v) => Response::ok(v),
            Err(e) => Response::err(e.to_string()),
        },
    }
}
