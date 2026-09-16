pub mod cache;
pub(crate) mod mcp_rpc;
pub(crate) mod mcp_service;
pub mod meta;
pub(crate) mod monitor_service;
pub(crate) mod operation_service;
pub(crate) mod operation_worker;
pub(crate) mod operations;
pub mod query;
pub mod query_state;
pub mod server;
pub(crate) mod tasks;
pub(crate) mod web_service;
pub(crate) mod worker_keys;

use anyhow::Result;
use std::sync::Arc;

use crate::runtime::RuntimeContext;

/// daemon 入口
///
/// 当 WX_DAEMON_MODE 环境变量设置时，main() 调用此函数
pub fn run() {
    let rt = tokio::runtime::Runtime::new().expect("无法创建 tokio runtime");
    if let Err(e) = rt.block_on(async_run()) {
        eprintln!("[daemon] 启动失败: {}", e);
        std::process::exit(1);
    }
}

async fn async_run() -> Result<()> {
    let runtime = if std::env::var("WX_DAEMON_BOOTSTRAP").as_deref() == Ok("1") {
        RuntimeContext::bootstrap()?
    } else {
        RuntimeContext::load()?
    };
    if let Ok(expected) = std::env::var("WX_CLI_EXPECTED_RUNTIME") {
        anyhow::ensure!(
            expected == runtime.id,
            "后台启动期间账号配置发生变化，拒绝连接错误账号"
        );
    }
    // 生命周期锁先于缓存初始化，防止同账号多个后台同时修改缓存。
    let _lifetime = runtime.lock("daemon.lock")?;
    tokio::fs::create_dir_all(runtime.cache_dir()).await?;

    let pid = std::process::id();

    eprintln!("[daemon] wx-daemon 启动 (PID {})", pid);

    // 加载配置
    let cfg = &runtime.config;
    eprintln!("[daemon] DB_DIR: {}", cfg.db_dir.display());

    let query = Arc::new(query_state::QueryState::new(runtime.clone()));
    let keys = worker_keys::Broker::new(runtime.clone(), query.clone());
    let bootstrap = runtime.is_bootstrap();
    let web = web_service::WebService::new(runtime.clone(), query.clone());
    let monitor = monitor_service::Service::new();
    let (shutdown, _) = tokio::sync::watch::channel(false);
    let (tasks, worker) = if bootstrap {
        (None, None)
    } else {
        let (tasks, receiver) = tasks::Service::new(runtime.clone(), query.clone(), keys.clone())?;
        let worker = tokio::spawn(tasks.clone().run_worker(receiver));
        (Some(tasks), Some(worker))
    };
    let operations = operation_service::Service::new(runtime.clone(), tasks.clone(), keys.clone());
    let handler_state = tasks.clone();
    let handler_operations = operations.clone();
    let handler_shutdown = shutdown.clone();
    let handler_web = web.clone();
    let handler_runtime = runtime.clone();
    let handler_query = query.clone();
    let handler_monitor = monitor.clone();
    let handler_keys = keys.clone();
    let handler = Arc::new(move |call, peer_pid| {
        let state = handler_state.clone();
        let operations = handler_operations.clone();
        let shutdown = handler_shutdown.clone();
        let web = handler_web.clone();
        let runtime = handler_runtime.clone();
        let query = handler_query.clone();
        let monitor = handler_monitor.clone();
        let keys = handler_keys.clone();
        async move {
            use crate::service::protocol::{Call, ServiceError, VERSION};
            if let Call::WorkerKeyRevision { request } = call {
                keys.verify_image_revision(peer_pid, request).await?;
                return Ok(serde_json::json!({"verified": true}));
            }
            if let Call::WorkerKeys { request } = call {
                let revision = keys.update(peer_pid, request).await?;
                if let Some(state) = state {
                    if state.refresh_redactor().await.is_err() {
                        state.request_shutdown();
                    }
                }
                return Ok(serde_json::json!({"revision": revision}));
            }
            if operation_service::Service::handles(&call) {
                return operations.dispatch(call).await;
            }
            if !bootstrap {
                match call {
                    Call::Monitor { request } => {
                        return monitor
                            .handle(request, |request| async move {
                                server::dispatch_state(request, &query).await
                            })
                            .await;
                    }
                    Call::Web { request } => return web.handle(*request).await,
                    Call::Mcp { request } => {
                        return mcp_rpc::dispatch(*request, runtime, query).await
                    }
                    _ => (),
                }
            }
            if let Some(state) = state {
                let info = matches!(&call, Call::Info {});
                let mut response = state.dispatch(call).await?;
                if info {
                    response["operation_api"] = serde_json::json!(1);
                }
                return Ok(response);
            }
            match call {
                Call::Info {} => {
                    Ok(serde_json::json!({"version":VERSION,"bootstrap":true,"operation_api":1}))
                }
                Call::Shutdown {} => {
                    shutdown.send_replace(true);
                    Ok(serde_json::json!({"stopping":true}))
                }
                _ => Err(ServiceError::new(
                    "account_required",
                    "Account service is unavailable during bootstrap",
                )),
            }
        }
    });
    let database_key_broker = keys.clone();
    let database_keys: crate::service::transport::DatabaseKeyHandler =
        Arc::new(move |request, peer_pid| {
            let broker = database_key_broker.clone();
            Box::pin(async move { broker.read_databases(peer_pid, request).await })
        });
    let image_key_broker = keys.clone();
    let image: crate::service::transport::ImageKeyHandler = Arc::new(move |request, peer_pid| {
        let broker = image_key_broker.clone();
        Box::pin(async move { broker.read_image(peer_pid, request).await })
    });
    let mut task_server = tokio::spawn(crate::service::transport::serve_with_worker_keys(
        runtime.clone(),
        handler,
        crate::service::transport::WorkerKeyHandlers {
            databases: database_keys,
            image,
        },
        shutdown.subscribe(),
    ));
    let server_query = query.clone();
    let mut query_server =
        tokio::spawn(async move { server::serve(server_query, &runtime.pipe_name()).await });
    let mut stop = tasks
        .as_ref()
        .map(|tasks| tasks.subscribe_shutdown())
        .unwrap_or_else(|| shutdown.subscribe());
    let monitor_reaper = tokio::spawn(monitor.clone().reap(shutdown.subscribe()));
    let idle_operations = operations.clone();
    let bootstrap_idle = async move {
        if !bootstrap {
            std::future::pending::<()>().await;
        }
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            if idle_operations.idle() {
                break;
            }
        }
    };
    let result = tokio::select! {
        result = &mut task_server => result.map_err(anyhow::Error::from).and_then(|result| result),
        result = &mut query_server => result.map_err(anyhow::Error::from).and_then(|result| result),
        _ = stop.changed() => Ok(()),
        _ = bootstrap_idle => Ok(()),
    };
    shutdown.send_replace(true);
    monitor.clear();
    let _ = monitor_reaper.await;
    if let Some(tasks) = tasks {
        tasks.request_shutdown();
    }
    keys.close();
    operations.shutdown().await;
    if let Err(error) = web.shutdown().await {
        eprintln!("[daemon] Web service shutdown failed: {error}");
    }
    if let Err(error) = mcp_service::shutdown().await {
        eprintln!("[daemon] MCP service shutdown failed: {error}");
    }
    // Workers first reap their process trees and persist terminal states.
    if let Some(worker) = worker {
        let _ = worker.await;
    }
    query_server.abort();
    if !query_server.is_finished() {
        let _ = query_server.await;
    }
    if !task_server.is_finished() {
        let _ = task_server.await;
    }
    query.shutdown().await;
    result
}
