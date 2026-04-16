use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

use crate::ipc::{Request, Response, WatchEvent};
use super::cache::DbCache;
use super::query::Names;

/// 启动 IPC server（Unix socket / Windows named pipe）
pub async fn serve(
    db: Arc<DbCache>,
    names: Arc<std::sync::RwLock<Names>>,
    watch_tx: broadcast::Sender<WatchEvent>,
) -> Result<()> {
    #[cfg(unix)]
    serve_unix(db, names, watch_tx).await?;
    #[cfg(windows)]
    serve_windows(db, names, watch_tx).await?;
    Ok(())
}

#[cfg(unix)]
async fn serve_unix(
    db: Arc<DbCache>,
    names: Arc<std::sync::RwLock<Names>>,
    watch_tx: broadcast::Sender<WatchEvent>,
) -> Result<()> {
    use tokio::net::UnixListener;
    let sock_path = crate::config::sock_path();

    // 删除旧 socket 文件
    if sock_path.exists() {
        let _ = tokio::fs::remove_file(&sock_path).await;
    }

    let listener = UnixListener::bind(&sock_path)?;
    // 设置权限 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))?;
    }

    eprintln!("[server] 监听 {}", sock_path.display());

    loop {
        let (stream, _) = listener.accept().await?;
        let db2 = Arc::clone(&db);
        let names2 = Arc::clone(&names);
        let tx2 = watch_tx.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection_unix(stream, db2, names2, tx2).await {
                eprintln!("[server] 连接处理错误: {}", e);
            }
        });
    }
}

#[cfg(unix)]
async fn handle_connection_unix(
    stream: tokio::net::UnixStream,
    db: Arc<DbCache>,
    names: Arc<std::sync::RwLock<Names>>,
    watch_tx: broadcast::Sender<WatchEvent>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let line = match lines.next_line().await? {
        Some(l) => l,
        None => return Ok(()),
    };

    // 解析请求
    let req: Request = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => {
            let resp = Response::err(format!("JSON 解析错误: {}", e));
            writer.write_all(resp.to_json_line()?.as_bytes()).await?;
            return Ok(());
        }
    };

    match req {
        Request::Watch => {
            // 流式模式：持续推送事件
            let mut rx = watch_tx.subscribe();
            let connected = WatchEvent::connected();
            writer.write_all(connected.to_json_line()?.as_bytes()).await?;

            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Ok(e) => {
                                if writer.write_all(e.to_json_line()?.as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => break,
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        // 心跳
                        let hb = WatchEvent::heartbeat();
                        if writer.write_all(hb.to_json_line()?.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
        other => {
            let resp = dispatch(other, &db, &names).await;
            writer.write_all(resp.to_json_line()?.as_bytes()).await?;
        }
    }
    Ok(())
}

#[cfg(windows)]
async fn serve_windows(
    db: Arc<DbCache>,
    names: Arc<std::sync::RwLock<Names>>,
    watch_tx: broadcast::Sender<WatchEvent>,
) -> Result<()> {
    use interprocess::local_socket::{
        tokio::prelude::*, GenericNamespaced, ListenerOptions,
    };

    let pipe_name = r"\\.\pipe\wx-cli-daemon";
    let name = pipe_name.to_ns_name::<GenericNamespaced>()?;
    let opts = ListenerOptions::new().name(name);
    let listener = opts.create_tokio()?;

    eprintln!("[server] 监听 {}", pipe_name);

    loop {
        let conn = listener.accept().await?;
        let db2 = Arc::clone(&db);
        let names2 = Arc::clone(&names);
        let tx2 = watch_tx.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection_generic(conn, db2, names2, tx2).await {
                eprintln!("[server] 连接处理错误: {}", e);
            }
        });
    }
}

async fn dispatch(
    req: Request,
    db: &DbCache,
    names: &std::sync::RwLock<Names>,
) -> Response {
    use crate::ipc::Request::*;
    use super::query;

    match req {
        Ping => Response::ok(serde_json::json!({ "pong": true })),
        Sessions { limit } => {
            // 在 await 前获取并复制所需数据，避免 RwLockGuard 跨 await
            let names_snapshot = match clone_names(names) {
                Ok(n) => n,
                Err(e) => return Response::err(e),
            };
            match query::q_sessions(db, &names_snapshot, limit).await {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        History { chat, limit, offset, since, until } => {
            let names_snapshot = match clone_names(names) {
                Ok(n) => n,
                Err(e) => return Response::err(e),
            };
            match query::q_history(db, &names_snapshot, &chat, limit, offset, since, until).await {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Search { keyword, chats, limit, since, until } => {
            let names_snapshot = match clone_names(names) {
                Ok(n) => n,
                Err(e) => return Response::err(e),
            };
            match query::q_search(db, &names_snapshot, &keyword, chats, limit, since, until).await {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Contacts { query, limit } => {
            let names_snapshot = match clone_names(names) {
                Ok(n) => n,
                Err(e) => return Response::err(e),
            };
            match query::q_contacts(&names_snapshot, query.as_deref(), limit).await {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Watch => Response::err("Watch 命令不应通过 dispatch 处理"),
    }
}

/// 克隆 Names 以避免 RwLockGuard 跨 await
fn clone_names(names: &std::sync::RwLock<Names>) -> Result<Names, String> {
    let guard = names.read().map_err(|_| "内部错误: names lock poisoned".to_string())?;
    Ok(Names {
        map: guard.map.clone(),
        md5_to_uname: guard.md5_to_uname.clone(),
        msg_db_keys: guard.msg_db_keys.clone(),
    })
}
