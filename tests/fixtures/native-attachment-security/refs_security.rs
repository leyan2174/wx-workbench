use crate::attachment_refs::{self as refs, Binding};
use attachment_refs_contract::wechat_content::{self as content, ErrorKind, MessageInput};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const ABC_MD5: &str = "900150983cd24fb0d6963f7d28e17f72";

fn input(body: &str) -> MessageInput<'_> {
    MessageInput {
        username: "synthetic_peer",
        source: "message/message_0.db",
        local_id: 7,
        create_time: 100,
        body,
    }
}
fn file_xml(hash: &str) -> String {
    format!("<msg><appmsg><type>6</type><title>report.txt</title><appattach><totallen>3</totallen></appattach><md5>{hash}</md5></appmsg></msg>")
}
fn record_xml(hash: &str) -> String {
    format!("<msg><appmsg><type>19</type><recorditem><![CDATA[<recordinfo><datalist><dataitem datatype='8'><datatitle>report.txt</datatitle><datasize>3</datasize><fullmd5>{hash}</fullmd5></dataitem></datalist></recordinfo>]]></recorditem></appmsg></msg>")
}
fn put(root: &Path, relative: &str, data: &[u8]) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, data).unwrap();
    path
}

#[test]
fn file_nested_hash_must_not_downgrade_to_wrong_heuristic_reference() {
    let root = tempfile::tempdir().unwrap();
    let file = put(root.path(), "msg/file/report.txt", b"bad");
    let canonical = content::parse_file_message(&input(&file_xml(ABC_MD5))).unwrap();
    assert_eq!(
        refs::find_reference(root.path(), &canonical)
            .unwrap_err()
            .kind,
        ErrorKind::HashMismatch
    );
    let body = file_xml(&format!("<value>{ABC_MD5}</value>"));
    let parsed = content::parse_file_message(&input(&body));
    if let Ok(meta) = &parsed {
        let found = refs::find_reference(root.path(), meta);
        println!("nested md5 parsed={meta:?}; resolution={:?}", found.as_ref().map(|value| value.as_ref().map(|r| serde_json::to_value(r).unwrap())));
    }
    assert_eq!(fs::read(&file).unwrap(), b"bad");
    assert!(
        parsed.is_err(),
        "含子元素的 MD5 必须拒绝，不能当成缺少 MD5 后返回不匹配文件"
    );
}

#[test]
fn record_nested_hash_must_not_downgrade_to_wrong_heuristic_reference() {
    let root = tempfile::tempdir().unwrap();
    let relative = format!(
        "msg/attach/{:x}/2026-09/Rec/card/F/0/report.txt",
        md5::compute(b"synthetic_peer")
    );
    let file = put(root.path(), &relative, b"bad");
    let canonical = content::parse_record_item(&input(&record_xml(ABC_MD5)), 0).unwrap();
    assert_eq!(
        refs::find_reference(root.path(), &canonical)
            .unwrap_err()
            .kind,
        ErrorKind::HashMismatch
    );
    let body = record_xml(&format!("<value>{ABC_MD5}</value>"));
    let parsed = content::parse_record_item(&input(&body), 0);
    if let Ok(meta) = &parsed {
        let found = refs::find_reference(root.path(), meta);
        println!("nested fullmd5 parsed={meta:?}; resolution={:?}", found.as_ref().map(|value| value.as_ref().map(|r| serde_json::to_value(r).unwrap())));
    }
    assert_eq!(fs::read(&file).unwrap(), b"bad");
    assert!(
        parsed.is_err(),
        "记录中的结构化 fullmd5 必须拒绝，不能静默降级"
    );
}

#[test]
fn mixed_hash_content_must_not_accept_only_valid_prefix() {
    let body = file_xml(&format!("{ABC_MD5}<suffix>not-a-hash</suffix>"));
    let parsed = content::parse_file_message(&input(&body));
    println!("mixed hash parse={parsed:?}");
    assert!(parsed.is_err(), "不能只使用首个文本节点来通过 MD5 验证");
}

#[test]
fn nested_size_must_not_remove_the_declared_size_constraint() {
    let body = file_xml("").replace(
        "<totallen>3</totallen>",
        "<totallen><value>500</value></totallen>",
    );
    let parsed = content::parse_file_message(&input(&body));
    println!("nested size parse={parsed:?}");
    assert!(
        parsed.is_err(),
        "已给出但结构错误的 totallen 不能变成大小未知"
    );
}

#[test]
fn controlled_account_roots_same_ids_do_not_cross_search() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let pa = put(a.path(), "msg/file/report.txt", b"abc");
    let pb = put(b.path(), "msg/file/report.txt", b"bad");
    let meta = content::parse_file_message(&input(&file_xml(ABC_MD5))).unwrap();
    let found = refs::find_reference(a.path(), &meta).unwrap().unwrap();
    assert_eq!(found.path, pa);
    assert_eq!(found.binding, Binding::Md5);
    assert_eq!(
        refs::find_reference(b.path(), &meta).unwrap_err().kind,
        ErrorKind::HashMismatch
    );
    let mut bytes = Vec::new();
    found.file().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"abc");
    drop(found);
    assert_eq!(fs::read(pa).unwrap(), b"abc");
    assert_eq!(fs::read(pb).unwrap(), b"bad");
}

#[test]
fn cross_message_record_candidates_without_hash_remain_ambiguous() {
    let root = tempfile::tempdir().unwrap();
    for card in ["record-one", "record-two"] {
        put(
            root.path(),
            &format!(
                "msg/attach/{:x}/2026-09/Rec/{card}/F/0/report.txt",
                md5::compute(b"synthetic_peer")
            ),
            b"abc",
        );
    }
    let meta = content::parse_record_item(&input(&record_xml("")), 0).unwrap();
    assert_eq!(
        refs::find_reference(root.path(), &meta).unwrap_err().kind,
        ErrorKind::Ambiguous
    );
}

#[test]
fn readonly_reference_locks_all_hardlink_aliases_on_windows() {
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), "msg/file/report.txt", b"abc");
    let alias = root.path().join("outside-alias.txt");
    fs::hard_link(&path, &alias).unwrap();
    let meta = content::parse_file_message(&input(&file_xml(ABC_MD5))).unwrap();
    let found = refs::find_reference(root.path(), &meta).unwrap().unwrap();
    #[cfg(windows)]
    assert!(fs::OpenOptions::new().write(true).open(&alias).is_err());
    drop(found);
    assert_eq!(fs::read(path).unwrap(), b"abc");
    assert_eq!(fs::read(alias).unwrap(), b"abc");
}

#[test]
fn traversal_and_ads_base_aliases_are_rejected_before_search() {
    let root = tempfile::tempdir().unwrap();
    let meta = content::parse_file_message(&input(&file_xml(ABC_MD5))).unwrap();
    for path in [
        root.path().join("child/.."),
        root.path().join("."),
        root.path().join("x:stream"),
        PathBuf::from("relative"),
    ] {
        assert_eq!(
            refs::find_reference(&path, &meta).unwrap_err().kind,
            ErrorKind::UnsafePath
        );
    }
}
