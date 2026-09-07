//! 真实加密账号、daemon 和 wx mcp 图像输出；不替代生产查询或解码器。
#![cfg(windows)]
#[path = "fixtures/mcp-image-runtime/images.rs"]
mod images;
#[allow(dead_code)]
#[path = "fixtures/mcp-readonly-runtime/support.rs"]
mod support;
use serde_json::{json, Value};
use std::{fs, path::Path};
use support::{Account, Mcp};

fn safe_failure(reply: Value, expected: &str) {
    assert_eq!(
        reply["result"],
        json!({"isError":true,"content":[{"type":"text","text":expected}]})
    );
    assert!(reply.get("error").is_none());
}

fn arguments(id: i64) -> Value {
    json!({"chat_name":"peer","local_id":id})
}

fn published(account: &Account, mcp: &mut Mcp, output: &Path, id: i64) -> Value {
    let value = mcp.data("decode_image", arguments(id));
    assert_eq!(value["exit_code"], 0);
    assert_eq!(value["status"], "published");
    let image = &value["image"];
    let path = Path::new(image["path"].as_str().unwrap());
    let source = Path::new(image["source_path"].as_str().unwrap());
    assert!(path.starts_with(output), "{value}");
    assert!(source.starts_with(account.root()), "{value}");
    assert_eq!(
        image["message"],
        json!({"username":"peer","source":"message/message_2.db","local_id":id,"create_time":id*100,"local_type":3})
    );
    let expected = images::bitmap(account.marker, id);
    assert_eq!(fs::read(path).unwrap(), expected);
    assert_eq!(image["size"], expected.len());
    assert_eq!(
        image["decoded_md5"],
        format!("{:x}", md5::compute(&expected))
    );
    assert_eq!(
        image["dat_md5"],
        format!("{:x}", md5::compute(fs::read(source).unwrap()))
    );
    assert_eq!(image["format"], "bmp");
    assert_eq!(
        image["decoder"],
        if id == 700 { "legacy_xor" } else { "v1_aes" }
    );
    value
}

#[test]
fn image_listing_returns_real_resource_and_encrypted_size_without_output_policy() {
    let home = tempfile::tempdir().unwrap();
    let mut account = Account::new(home.path(), "A");
    images::seed(&account);
    let before = account.snapshot();
    account.start();
    let mut mcp = account.mcp();
    mcp.ready();
    let list = mcp.data("get_chat_images", json!({"chat_name":"peer","limit":3}));
    assert_eq!(list["count"], 3);
    for (row, (id, hash)) in list["images"].as_array().unwrap().iter().zip([
        (702, "33333333333333333333333333333333"),
        (701, "22222222222222222222222222222222"),
        (700, "11111111111111111111111111111111"),
    ]) {
        let dat = account.root().join(format!(
            "msg/attach/{:x}/2026-09/Img/{hash}.dat",
            md5::compute(b"peer")
        ));
        let size = fs::metadata(&dat).unwrap().len();
        assert_eq!(
            row,
            &json!({"local_id":id,"create_time":id*100,"md5":hash,"size":size,
            "resource_status":"found","size_status":"available","size_kind":"encrypted_dat_metadata",
            "binding":"exact_resource_standard_filename_metadata"})
        );
        if id == 701 {
            assert_ne!(size, images::bitmap("A", id).len() as u64);
        }
    }
    assert!(!list.to_string().contains(account.root().to_str().unwrap()));
    let second = mcp.data(
        "get_chat_images",
        json!({"chat_name":"peer","limit":1,"offset":1}),
    );
    assert_eq!(second["images"][0], list["images"][1]);
    let hash = "11111111111111111111111111111111";
    let directory = account.root().join(format!(
        "msg/attach/{:x}/2026-09/Img",
        md5::compute(b"peer")
    ));
    let dat = directory.join(format!("{hash}.dat"));
    let variant = directory.join(format!("{hash}_t.dat"));
    fs::write(&variant, b"synthetic second candidate").unwrap();
    let changed = account.snapshot();
    let filter = json!({"chat_name":"peer","since":70000,"until":70000});
    let ambiguous = mcp.data("get_chat_images", filter.clone());
    assert_eq!(ambiguous["images"][0]["md5"], hash);
    assert_eq!(ambiguous["images"][0]["size_status"], "ambiguous");
    assert!(ambiguous["images"][0]["size"].is_null());
    assert_eq!(account.snapshot(), changed);
    fs::remove_file(&variant).unwrap();
    let removed = dat.with_extension("removed");
    fs::rename(&dat, &removed).unwrap();
    let changed = account.snapshot();
    let missing = mcp.data("get_chat_images", filter);
    assert_eq!(missing["images"][0]["md5"], hash);
    assert_eq!(missing["images"][0]["size_status"], "missing");
    assert!(missing["images"][0]["size"].is_null());
    assert_eq!(account.snapshot(), changed);
    fs::rename(&removed, &dat).unwrap();
    mcp.finish();
    assert_eq!(account.snapshot(), before);
}

#[test]
fn image_mcp_uses_real_accounts_decodes_exact_bytes_and_never_overwrites() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), "A");
    let mut b = Account::new(home.path(), "B");
    images::seed(&a);
    images::seed(&b);
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    let output = tempfile::tempdir().unwrap();
    // 不启动 daemon：初始化/列举必须不碰账号密钥或创建解密输出。
    let config_path = a.root().join("config.json");
    let keys_path = a.root().join("keys.json");
    let saved_config = fs::read(&config_path).unwrap();
    let saved_keys = fs::read(&keys_path).unwrap();
    fs::write(
        &config_path,
        b"invalid config: initialize must not read this",
    )
    .unwrap();
    fs::write(&keys_path, b"invalid keys: initialize must not read this").unwrap();
    let warm_before = a.snapshot();
    let mut warm = a.mcp_with_args(&["--media-output-root", output.path().to_str().unwrap()]);
    warm.ready();
    let list = warm.rpc("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 17);
    assert_eq!(tools[14]["name"], "decode_image");
    assert!(!home.path().join("accounts").exists());
    assert!(!a.root().join("decrypted").exists());
    assert_eq!(a.snapshot(), warm_before);
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    warm.finish();
    fs::write(config_path, saved_config).unwrap();
    fs::write(keys_path, saved_keys).unwrap();
    assert_eq!(a.snapshot(), before_a);
    a.start();
    b.start();
    let mut missing = a.mcp();
    missing.ready();
    safe_failure(
        missing.call("decode_image", arguments(700)),
        "Query backend unavailable",
    );
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    missing.finish();
    let mut ma = a.mcp_with_args(&["--media-output-root", output.path().to_str().unwrap()]);
    let mut mb = b.mcp_with_args(&["--media-output-root", output.path().to_str().unwrap()]);
    ma.ready();
    mb.ready();
    let first = published(&a, &mut ma, output.path(), 700);
    let first_path = Path::new(first["image"]["path"].as_str().unwrap());
    let saved = fs::read(first_path).unwrap();
    // 同一资源 hash 和输出根，但实际像素不同：两个结果共存，B 不能覆盖 A。
    let second = published(&b, &mut mb, output.path(), 700);
    assert_ne!(first["image"]["path"], second["image"]["path"]);
    let duplicate = ma.call("decode_image", arguments(700));
    assert_eq!(fs::read(first_path).unwrap(), saved);
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 2);
    // 先证明失败后同一 MCP 会话仍可导出，再严格核验业务错误分类。
    published(&a, &mut ma, output.path(), 701);
    eprintln!("Same MCP session exported 701 after duplicate 700; exact output bytes verified");
    safe_failure(duplicate, "Query failed");
    safe_failure(ma.call("decode_image", arguments(702)), "Query failed");
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 3);
    for field in [
        "output_root",
        "media_output_root",
        "image_key_file",
        "source",
    ] {
        let mut args = arguments(700);
        args[field] = json!("forbidden");
        assert_eq!(ma.call("decode_image", args)["error"]["code"], -32602);
    }
    ma.finish();
    mb.finish();
    let b_output = tempfile::tempdir().unwrap();
    let mut mb = b.mcp_with_args(&["--media-output-root", b_output.path().to_str().unwrap()]);
    mb.ready();
    published(&b, &mut mb, b_output.path(), 700);
    published(&b, &mut mb, b_output.path(), 701);
    mb.finish();
    assert_eq!(fs::read(first_path).unwrap(), saved);
    assert_eq!(a.snapshot(), before_a);
    assert_eq!(b.snapshot(), before_b);
}
