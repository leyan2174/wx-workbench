use anyhow::Result;
use std::sync::Arc;
#[cfg(test)]
use tokio::io::AsyncBufRead;
use tokio::io::BufReader;

use super::cache::DbCache;
use super::query::Names;
use super::query_state::QueryState;
use crate::ipc::{Request, Response};

fn query_response(result: Result<serde_json::Value>) -> Response {
    match result {
        Ok(value) => Response::ok(value),
        Err(error) => query_error(error),
    }
}

fn query_error(error: anyhow::Error) -> Response {
    let ambiguous = matches!(
        error.downcast_ref::<crate::business::contacts::Error>(),
        Some(crate::business::contacts::Error::Ambiguous)
    ) || matches!(
        error.downcast_ref::<crate::business::messages::Error>(),
        Some(crate::business::messages::Error::Ambiguous)
    );
    let mut response = Response::err(error.to_string());
    if ambiguous {
        response.data = serde_json::json!({
            "status": "ambiguous", "error_code": "ambiguous_identity", "exit_code": 2
        });
    }
    response
}

#[cfg(test)]
mod query_error_tests {
    use super::*;

    #[test]
    fn typed_ambiguity_survives_context_but_error_text_is_not_a_classifier() {
        for error in [
            anyhow::Error::new(crate::business::contacts::Error::Ambiguous).context("tag lookup"),
            anyhow::Error::new(crate::business::messages::Error::Ambiguous).context("chat lookup"),
        ] {
            let response = query_error(error);
            assert!(!response.ok);
            assert_eq!(response.data["status"], "ambiguous");
            assert_eq!(response.data["exit_code"], 2);
            assert!(response.require_success().is_err());
        }
        let response = query_error(anyhow::anyhow!("ambiguous tag name"));
        assert!(response.data.is_null());
        assert!(response.require_success().is_err());
    }
}

#[cfg(test)]
#[path = "server_contacts_tests.rs"]
mod contacts_tests;

#[cfg(test)]
#[path = "server_chat_tests.rs"]
mod chat_tests;

/// 启动 IPC server（Windows named pipe）
pub async fn serve(state: Arc<QueryState>, pipe_name: &str) -> Result<()> {
    #[cfg(windows)]
    serve_windows(state, pipe_name).await?;
    Ok(())
}

#[cfg(windows)]
async fn serve_windows(state: Arc<QueryState>, pipe_name: &str) -> Result<()> {
    use interprocess::local_socket::{tokio::prelude::*, GenericNamespaced, ListenerOptions};

    // 库自动补上 Windows 管道前缀；名称统一来自启动时固定的账号上下文。
    let name = pipe_name.to_ns_name::<GenericNamespaced>()?;
    let opts = ListenerOptions::new().name(name);
    let listener = opts.create_tokio()?;
    let runtime_id = pipe_name
        .strip_prefix("wx-cli-v2-")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid query runtime pipe"))?
        .to_owned();
    let connections = Arc::new(tokio::sync::Semaphore::new(64));

    eprintln!("[server] 监听账号管道 {pipe_name}");

    loop {
        // 先取得连接许可再接收，避免派生无界等待任务。
        let permit = Arc::clone(&connections).acquire_owned().await?;
        let conn = listener.accept().await?;
        let state = Arc::clone(&state);
        let runtime_id = runtime_id.clone();

        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = handle_connection_windows(conn, state, &runtime_id).await {
                eprintln!("[server] 连接处理错误: {}", e);
            }
        });
    }
}

#[cfg(windows)]
async fn handle_connection_windows(
    conn: interprocess::local_socket::tokio::Stream,
    state: Arc<QueryState>,
    runtime_id: &str,
) -> Result<()> {
    use crate::ipc::{QueryEnvelope, QueryHello, QUERY_VERSION};
    use crate::service::transport::{self, framing};
    // One deadline includes handshake, request, dispatch and response writing.
    tokio::time::timeout(std::time::Duration::from_secs(3600), async {
        let (reader, mut writer) = tokio::io::split(conn);
        let hello = transport::encode(
            &QueryHello {
                version: QUERY_VERSION,
                runtime_id: runtime_id.into(),
            },
            1023,
        )?;
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            framing::write_line(&mut writer, &hello, 1024).await?;
            framing::line(&mut BufReader::new(reader), MAX_REQUEST_FRAME_BYTES).await
        })
        .await
        .map_err(|_| framing::FrameError::Timeout)??;
        let envelope: QueryEnvelope =
            serde_json::from_slice(&line).map_err(|_| framing::FrameError::Protocol)?;
        if envelope.version != QUERY_VERSION || envelope.runtime_id != runtime_id {
            return Err(framing::FrameError::Protocol.into());
        }
        framing::budget(envelope.response_limit)?;
        let limit = envelope
            .response_limit
            .min(crate::ipc::query_response_limit(&envelope.request));
        let resp = dispatch_state(envelope.request, &state).await;
        let reply = crate::ipc::QueryReply::Response {
            version: QUERY_VERSION,
            runtime_id: runtime_id.into(),
            response: resp,
        };
        let bytes = match transport::encode(&reply, limit.saturating_sub(1)) {
            Ok(bytes) => bytes,
            Err(error)
                if matches!(
                    error.downcast_ref::<framing::FrameError>(),
                    Some(framing::FrameError::Oversize)
                ) =>
            {
                transport::encode(
                    &crate::ipc::QueryReply::Oversize {
                        version: QUERY_VERSION,
                        runtime_id: runtime_id.into(),
                    },
                    limit.saturating_sub(1),
                )?
            }
            Err(error) => return Err(error),
        };
        framing::write_line(&mut writer, &bytes, limit).await?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|_| framing::FrameError::Timeout)?
}

pub(super) async fn dispatch_state(req: Request, state: &QueryState) -> Response {
    if matches!(req, Request::Ping) {
        return Response::ok(serde_json::json!({ "pong": true }));
    }
    match state.snapshot().await {
        Ok(lease) => {
            let response = match req {
                Request::Extract {
                    attachment_id,
                    output,
                    overwrite,
                } => query_response(
                    super::query::q_extract(
                        lease.db(),
                        state.runtime(),
                        lease.key_material(),
                        &attachment_id,
                        &output,
                        overwrite,
                    )
                    .await,
                ),
                req => dispatch(req, lease.db(), lease.names()).await,
            };
            drop(lease);
            response
        }
        Err(error) => query_initialization_failure(&error),
    }
}

fn query_initialization_failure(error: &anyhow::Error) -> Response {
    use crate::{ipc::outcome::KeyStoreDiagnostic as Diagnostic, key_store::Error};
    let Some(error) = error.downcast_ref::<Error>() else {
        return Response::err(
            "Query initialization failed; check account configuration and keys, then retry",
        );
    };
    let diagnostic = match error {
        Error::Missing => Diagnostic::Missing,
        Error::LegacyMigrationRequired => Diagnostic::LegacyMigrationRequired,
        Error::Invalid => Diagnostic::Invalid,
        Error::WrongAccount => Diagnostic::WrongAccount,
        Error::Protection => Diagnostic::Protection,
        Error::Conflict => Diagnostic::Conflict,
        Error::Busy => Diagnostic::Busy,
        Error::Io => Diagnostic::Io,
    };
    let mut response = Response::err(diagnostic.message());
    response.data = serde_json::json!({"error_code":diagnostic.code()});
    response
}

#[test]
fn typed_key_store_initialization_errors_are_distinct_and_private() {
    use crate::{ipc::outcome::KeyStoreDiagnostic as Diagnostic, key_store::Error};
    for (error, diagnostic) in [
        (Error::Missing, Diagnostic::Missing),
        (
            Error::LegacyMigrationRequired,
            Diagnostic::LegacyMigrationRequired,
        ),
        (Error::Invalid, Diagnostic::Invalid),
        (Error::WrongAccount, Diagnostic::WrongAccount),
        (Error::Protection, Diagnostic::Protection),
        (Error::Conflict, Diagnostic::Conflict),
        (Error::Busy, Diagnostic::Busy),
        (Error::Io, Diagnostic::Io),
    ] {
        let response = query_initialization_failure(
            &anyhow::Error::new(error).context("SYNTHETIC_PRIVATE_KEY"),
        );
        assert!(!response.ok);
        assert_eq!(response.error.as_deref(), Some(diagnostic.message()));
        assert_eq!(response.data["error_code"], diagnostic.code());
        assert_eq!(
            response.require_success().unwrap_err().diagnostic(),
            Some(diagnostic)
        );
        assert!(!serde_json::to_string(&response)
            .unwrap()
            .contains("SYNTHETIC_PRIVATE_KEY"));
    }
    let response = query_initialization_failure(&anyhow::anyhow!("SYNTHETIC_PRIVATE_KEY"));
    assert!(!serde_json::to_string(&response)
        .unwrap()
        .contains("SYNTHETIC_PRIVATE_KEY"));
}

pub(super) const MAX_REQUEST_FRAME_BYTES: usize = 64 * 1024;

#[cfg(test)]
pub(super) async fn read_initial_request_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_request_frame(reader),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Initial request frame timed out",
        )
    })?
}

#[cfg(test)]
pub(super) async fn read_request_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    use crate::service::transport::framing::{self, FrameError};
    let frame = match framing::line(reader, MAX_REQUEST_FRAME_BYTES).await {
        Ok(frame) => frame,
        Err(FrameError::Eof) => return Ok(None),
        Err(error) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
    };
    String::from_utf8(frame.to_vec())
        .map(|s| s.trim_end_matches(['\r', '\n']).to_owned())
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

    let structured_decode = matches!(
        &req,
        DecodeTransfer { .. }
            | DecodeLocation { .. }
            | DecodeRefer { .. }
            | DecodeFileMessage { .. }
            | DecodeRecordItem { .. }
    );
    let mut response = match req {
        Ping => Response::ok(serde_json::json!({ "pong": true })),
        LatencyProbe { limit } => match db.latency_probe(limit).await {
            Ok(probe) => Response::from_result(serde_json::to_value(probe)),
            Err(error) => Response::err(error.to_string()),
        },
        ResolveChat { chat } => match query::q_resolve_chat(db, &names_arc, &chat).await {
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
                Err(error) => query_error(error),
            }
        }
        DecodeRefer {
            chat,
            local_id,
            create_time,
        } => query_response(
            query::q_decode_refer(db, &names_arc, &chat, local_id, create_time).await,
        ),
        DecodeFileMessage {
            chat,
            local_id,
            create_time,
        } => query_response(
            query::mcp_attachments::q_attachment_reference(
                db,
                &names_arc,
                &chat,
                local_id,
                create_time,
                None,
            )
            .await,
        ),
        DecodeRecordItem {
            chat,
            local_id,
            item_index,
            create_time,
        } => query_response(
            query::mcp_attachments::q_attachment_reference(
                db,
                &names_arc,
                &chat,
                local_id,
                create_time,
                Some(item_index),
            )
            .await,
        ),
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
                // 未分类的导出失败与明确的消息缺失分开；保持传输可用，不泄露错误链。
                Err(_) => Response::ok(serde_json::json!({
                    "exit_code": 3,
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
                let username = query::q_resolve_chat(db, &names_arc, &chat).await?;
                let page = query::mcp_voice::q_voice_messages(
                    db,
                    &crate::business::voice::catalog::Query {
                        username,
                        limit,
                        offset,
                        since,
                        until,
                    },
                )
                .await?;
                let rows = crate::adapters::wechat::media::voice_catalog::legacy_rows(&page)?;
                Ok::<_, anyhow::Error>(serde_json::json!({"voices": rows, "count": rows.len()}))
            }
            .await;
            query_response(result)
        }
        ExportChatList => query_response(query::q_export_chat_list(db, &names_arc).await),
        ExportDirectoryCatalog => {
            query_response(query::q_export_directory_catalog(db, &names_arc).await)
        }
        ExportDelta {
            username,
            start,
            end,
        } => query_response(
            query::q_export_delta_username(db, &names_arc, username, Some(start), end).await,
        ),
        ExportChatByUsername { username } => {
            query_response(query::q_export_username(db, &names_arc, username).await)
        }
        ExportDirectoryByUsername { username } => {
            query_response(query::q_export_directory_by_username(db, &names_arc, username).await)
        }
        ExportChat { chat } => query_response(query::q_export_chat(db, &names_arc, &chat).await),
        DecodeTransfer {
            chat,
            local_id,
            create_time,
        } => query_response(
            query::q_decode(
                db,
                &names_arc,
                &chat,
                local_id,
                create_time,
                query::DecodeKind::Transfer,
            )
            .await,
        ),
        DecodeLocation {
            chat,
            local_id,
            create_time,
        } => query_response(
            query::q_decode(
                db,
                &names_arc,
                &chat,
                local_id,
                create_time,
                query::DecodeKind::Location,
            )
            .await,
        ),
        Sessions {
            limit,
            with_meta,
            debug_source,
        } => {
            query_response(query::q_sessions(db, &names_arc, limit, with_meta, debug_source).await)
        }
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
                query::HistoryQuery {
                    page: query::MessagePage { limit, offset },
                    filter: query::MessageFilter {
                        since,
                        until,
                        msg_type,
                    },
                    meta: query::MetaOptions {
                        with_meta,
                        debug_source,
                    },
                    msg_types: msg_types.as_deref(),
                    oldest_first,
                },
            )
            .await
            {
                Ok(v) => Response::ok(v),
                Err(e)
                    if e.downcast_ref::<crate::business::messages::Error>()
                        == Some(&crate::business::messages::Error::Limit) =>
                {
                    let mut response =
                        Response::err("Message read budget exceeded; use a smaller page");
                    response.data = serde_json::json!({"error_code": "query_read_limit_exceeded"});
                    response
                }
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
                query::MessageFilter {
                    since,
                    until,
                    msg_type,
                },
                query::MetaOptions {
                    with_meta,
                    debug_source,
                },
            )
            .await
            {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Contacts(request) => query_response(
            query::q_contacts(&names_arc, request.query.as_deref(), request.limit).await,
        ),
        Unread {
            limit,
            filter,
            with_meta,
            debug_source,
        } => query_response(
            query::q_unread(db, &names_arc, limit, filter, with_meta, debug_source).await,
        ),
        Members { chat } => query_response(query::q_members(db, &names_arc, &chat).await),
        NewMessages {
            state,
            limit,
            with_meta,
            debug_source,
        } => query_response(
            query::q_new_messages(db, &names_arc, state, limit, with_meta, debug_source).await,
        ),
        Favorites {
            limit,
            fav_type,
            query,
        } => query_response(query::q_favorites(db, limit, fav_type, query).await),
        Stats {
            chat,
            since,
            until,
            with_meta,
            debug_source,
        } => query_response(
            query::q_stats(db, &names_arc, &chat, since, until, with_meta, debug_source).await,
        ),
        SnsNotifications {
            limit,
            since,
            until,
            include_read,
        } => query_response(
            query::q_sns_notifications(db, &names_arc, limit, since, until, include_read).await,
        ),
        SnsFeed {
            limit,
            since,
            until,
            user,
        } => query_response(
            query::q_sns_feed(db, &names_arc, limit, since, until, user.as_deref()).await,
        ),
        SnsSearch {
            keyword,
            limit,
            since,
            until,
            user,
        } => query_response(
            query::q_sns_search(
                db,
                &names_arc,
                &keyword,
                limit,
                since,
                until,
                user.as_deref(),
            )
            .await,
        ),
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
        } => query_response(
            query::q_biz_articles(db, &names_arc, limit, account, since, until, unread).await,
        ),
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
            let options = query::AttachmentQuery {
                kinds,
                page: query::MessagePage { limit, offset },
                since,
                until,
                meta: query::MetaOptions {
                    with_meta,
                    debug_source,
                },
            };
            let result = if image_metadata {
                query::q_attachments_with_image_metadata(db, &names_arc, &chat, options).await
            } else {
                query::q_attachments(db, &names_arc, &chat, options).await
            };
            query_response(result)
        }
        Extract { .. } => {
            Response::err("Attachment extraction requires an account-bound query lease")
        }
    };
    if structured_decode && response.data["exit_code"] == 2 {
        response.data["status"] = serde_json::json!("ambiguous");
        response.data["error_code"] = serde_json::json!("ambiguous_identity");
    }
    response
}
