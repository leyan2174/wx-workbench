use mcp_voice_host::{
    ipc::{Request, Response},
    mcp_service,
    service::mcp::{unpack, Call, HostSettings},
    protocol::{CallContext, DispatchError},
    runtime::RuntimeContext,
};
use std::{sync::mpsc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancels_and_drains_before_releasing_account_lock() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "db_dir": root.path().join("db"), "keys_file":root.path().join("keys.json"),
            "decrypted_dir":root.path().join("decrypted"), "wechat_process":"Synthetic.exe",
        }))
        .unwrap(),
    )
    .unwrap();
    std::env::set_var("WX_CLI_CONFIG", &path);
    std::env::set_var("WX_CLI_HOME", root.path().join("runtime"));
    let runtime = RuntimeContext::load().unwrap();
    let call = Call {
        session: "shutdown-synthetic".into(),
        open_session: true,
        owner_pid: std::process::id(),
        runtime_id: runtime.id.clone(),
        host: HostSettings::default(),
        budget: CallContext::default().budget().unwrap(),
        request: Some(Box::new(Request::ContactTags)),
    };
    let (started, ready) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let worker_runtime = runtime.clone();
    let worker_call = call.clone();
    let worker = std::thread::spawn(move || {
        mcp_service::dispatch(worker_call, &worker_runtime, |_, context, _| {
            started.send(()).unwrap();
            while context.check().is_ok() {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(context.check(), Err(DispatchError::Cancelled));
            std::thread::sleep(Duration::from_millis(50));
            finished.send(()).unwrap();
            Ok(Response::ok(serde_json::json!({})))
        })
    });
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
    mcp_service::shutdown().await.unwrap();
    done.try_recv().unwrap();
    assert!(unpack(worker.join().unwrap()).is_err());
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_ok());
    assert_eq!(
        unpack(mcp_service::dispatch(call, &runtime, |_, _, _| {
            panic!("shutdown accepted another call")
        }))
        .unwrap_err(),
        DispatchError::Unavailable
    );
}
