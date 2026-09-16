use super::*;
use crate::adapters::wechat::media::attachment_content::{
    parse_file_message, parse_record_item, AttachmentMetadata, ErrorKind, MessageInput,
};
use serde_json::{json, Value};
use std::io::Cursor;

fn input(body: &str) -> MessageInput<'_> {
    MessageInput {
        username: "wxid_synthetic",
        source: "message/message_12.db",
        local_id: 7,
        create_time: 123,
        body,
    }
}
fn file_xml(name: &str, size: &str, md5: &str) -> String {
    format!("<msg><appmsg><type>6</type><title>{name}</title><appattach><fileext>txt</fileext><totallen>{size}</totallen></appattach><md5>{md5}</md5></appmsg></msg>")
}
fn record_xml(items: &str) -> String {
    format!("<msg><appmsg><type>19</type><recorditem><![CDATA[<recordinfo><datalist>{items}</datalist></recordinfo>]]></recorditem></appmsg></msg>")
}
fn item(kind: &str, name: &str, size: &str, md5: &str) -> String {
    format!("<dataitem datatype='{kind}'><datatitle>{name}</datatitle><datasize>{size}</datasize><fullmd5>{md5}</fullmd5><datadesc>文本 内容</datadesc></dataitem>")
}
fn file_meta(name: &str, size: &str, md5: &str) -> AttachmentMetadata {
    parse_file_message(&input(&file_xml(name, size, md5))).unwrap()
}
fn record_meta(kind: &str, name: &str, size: &str, md5: &str) -> AttachmentMetadata {
    parse_record_item(&input(&record_xml(&item(kind, name, size, md5))), 0).unwrap()
}
fn put(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}
fn rec_path(card: &str, tail: &str) -> String {
    format!(
        "msg/attach/{:x}/2026-09/Rec/{card}/{tail}",
        md5::compute(b"wxid_synthetic")
    )
}
fn code(result: Result<Option<FileReference>>) -> ErrorKind {
    result.unwrap_err().kind
}
const ABC_MD5: &str = "900150983cd24fb0d6963f7d28e17f72";

#[test]
fn scalar_constraints_reject_nested_and_mixed_elements() {
    for (tag, value) in [
        ("md5", ABC_MD5),
        ("totallen", "3"),
        ("fullmd5", ABC_MD5),
        ("datasize", "3"),
    ] {
        for malformed in [
            format!("<value>{value}</value>"),
            format!("{value}<extra/>"),
            format!("<extra/>{value}"),
        ] {
            let body = if matches!(tag, "md5" | "totallen") {
                file_xml("report.txt", "3", ABC_MD5)
            } else {
                record_xml(&item("8", "report.txt", "3", ABC_MD5))
            };
            let body = body.replace(
                &format!("<{tag}>{value}</{tag}>"),
                &format!("<{tag}>{malformed}</{tag}>"),
            );
            let result = if matches!(tag, "md5" | "totallen") {
                parse_file_message(&input(&body))
            } else {
                parse_record_item(&input(&body), 0)
            };
            assert_eq!(
                result.unwrap_err().kind,
                ErrorKind::InvalidMetadata,
                "{tag}: {malformed}"
            );
        }
    }
}

#[test]
fn scalar_constraints_accept_cdata_and_complete_split_text() {
    for (size, hash) in [
        ("<![CDATA[3]]>".to_owned(), format!("<![CDATA[{ABC_MD5}]]>")),
        (
            "<!--size-->3".to_owned(),
            format!("{}<!--split-->{}", &ABC_MD5[..16], &ABC_MD5[16..]),
        ),
    ] {
        let file = parse_file_message(&input(&file_xml("report.txt", &size, &hash))).unwrap();
        let inner = format!(
            "<recordinfo><datalist>{}</datalist></recordinfo>",
            item("8", "report.txt", &size, &hash)
        );
        let escaped = inner
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let body = format!(
            "<msg><appmsg><type>19</type><recorditem>{escaped}</recorditem></appmsg></msg>"
        );
        let record = parse_record_item(&input(&body), 0).unwrap();
        for meta in [file, record] {
            assert_eq!(meta.expected_size, Some(3));
            assert_eq!(meta.expected_md5.as_deref(), Some(ABC_MD5));
        }
    }
    let hash = format!("{ABC_MD5}<!--split-->bad");
    assert_eq!(
        parse_file_message(&input(&file_xml("report.txt", "3", &hash)))
            .unwrap_err()
            .kind,
        ErrorKind::InvalidHash
    );
}

#[test]
fn scalar_constraints_preserve_absent_empty_and_rich_description() {
    for remove in [false, true] {
        let mut file = file_xml("report.txt", "", "");
        let mut record = record_xml(&item("8", "report.txt", "", ""));
        if remove {
            file = file
                .replace("<totallen></totallen>", "")
                .replace("<md5></md5>", "");
            record = record
                .replace("<datasize></datasize>", "")
                .replace("<fullmd5></fullmd5>", "");
        }
        record = record.replace("<datadesc>", "<datadesc><b>rich</b>");
        for meta in [
            parse_file_message(&input(&file)).unwrap(),
            parse_record_item(&input(&record), 0).unwrap(),
        ] {
            assert_eq!(meta.expected_size, None);
            assert_eq!(meta.expected_md5, None);
        }
    }
}

#[test]
fn matches_legacy_ast_metadata_golden() {
    let golden: Value = serde_json::from_str(include_str!("golden.json")).unwrap();
    for case in golden["cases"].as_array().unwrap() {
        let input = MessageInput {
            username: case["username"].as_str().unwrap(),
            source: case["source"].as_str().unwrap(),
            local_id: case["local_id"].as_i64().unwrap(),
            create_time: case["create_time"].as_i64().unwrap(),
            body: case["body"].as_str().unwrap(),
        };
        let parsed = if case["mode"] == "file" {
            parse_file_message(&input)
        } else {
            parse_record_item(&input, case["index"].as_i64().unwrap())
        }
        .unwrap();
        let mut value = serde_json::to_value(parsed).unwrap();
        let identity = value.as_object_mut().unwrap().remove("identity").unwrap();
        assert_eq!(identity["source"], case["source"]);
        assert_eq!(value, case["expected"], "{}", case["name"]);
    }
    eprintln!(
        "AST metadata golden cases: {}",
        golden["cases"].as_array().unwrap().len()
    );
}

#[test]
fn message_identity_is_explicit_and_preserved() {
    let body = file_xml("report.txt", "3", "");
    let mut data = input(&body);
    for source in ["message/message_12.db", "MESSAGE\\Message_255.db"] {
        data.source = source;
        assert_eq!(
            parse_file_message(&data).unwrap().identity.source,
            source.replace('\\', "/")
        );
    }
    for source in [
        "message_12.db",
        "../message/message_0.db",
        "message/media_1.db",
        "message/message_1.db:ads",
        "message/message_x.db",
    ] {
        data.source = source;
        assert_eq!(
            parse_file_message(&data).unwrap_err().kind,
            ErrorKind::InvalidIdentity
        );
    }
    data = input(&body);
    data.local_id = 0;
    assert_eq!(
        parse_file_message(&data).unwrap_err().kind,
        ErrorKind::InvalidIdentity
    );
    data.local_id = 1;
    data.username = "bad\nname";
    assert_eq!(
        parse_file_message(&data).unwrap_err().kind,
        ErrorKind::InvalidIdentity
    );
}

#[test]
fn files_require_explicit_type_six_and_appattach() {
    for kind in [5, 19, 33, 36, 2000] {
        let body = file_xml("report.txt", "3", "")
            .replace("<type>6</type>", &format!("<type>{kind}</type>"));
        assert_eq!(
            parse_file_message(&input(&body)).unwrap_err().kind,
            ErrorKind::WrongType
        );
    }
    let body = "<msg><appmsg><type>6</type><title>r.txt</title></appmsg></msg>";
    assert_eq!(
        parse_file_message(&input(body)).unwrap_err().kind,
        ErrorKind::InvalidMetadata
    );
}

#[test]
fn duplicate_or_namespaced_fields_are_not_silently_chosen() {
    let body = file_xml("report.txt", "3", "");
    for bad in [
        body.replace("<type>6</type>", "<type>6</type><type>6</type>"),
        body.replace("<title>", "<title>other.txt</title><title>"),
        body.replace(
            "<totallen>3</totallen>",
            "<totallen>3</totallen><totallen>4</totallen>",
        ),
    ] {
        assert_eq!(
            parse_file_message(&input(&bad)).unwrap_err().kind,
            ErrorKind::InvalidMetadata
        );
    }
    let body = "<msg xmlns='urn:test'><appmsg><type>6</type><title>r.txt</title><appattach/></appmsg></msg>";
    assert_eq!(
        parse_file_message(&input(body)).unwrap_err().kind,
        ErrorKind::InvalidMetadata
    );
}

#[test]
fn invalid_hash_never_downgrades() {
    for bad in [
        "abc",
        "gggggggggggggggggggggggggggggggg",
        "900150983cd24fb0d6963f7d28e17f7x2",
    ] {
        assert_eq!(
            parse_file_message(&input(&file_xml("r.txt", "3", bad)))
                .unwrap_err()
                .kind,
            ErrorKind::InvalidHash
        );
        assert_eq!(
            parse_record_item(&input(&record_xml(&item("8", "r.txt", "3", bad))), 0)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidHash
        );
    }
    assert_eq!(
        file_meta("r.txt", "3", &ABC_MD5.to_uppercase())
            .expected_md5
            .as_deref(),
        Some(ABC_MD5)
    );
    let mut meta = file_meta("r.txt", "3", "");
    meta.expected_md5 = Some("bad".into());
    assert_eq!(
        code(find_reference(Path::new("missing"), &meta)),
        ErrorKind::InvalidHash
    );
}

#[test]
fn sizes_do_not_silently_accept_invalid_or_negative_values() {
    for bad in ["-1", "1.5", "no", "_1", "1__2", "18446744073709551616"] {
        assert_eq!(
            parse_file_message(&input(&file_xml("r.txt", bad, "")))
                .unwrap_err()
                .kind,
            ErrorKind::InvalidMetadata
        );
    }
    assert_eq!(file_meta("r.txt", "0", "").expected_size, None);
    assert_eq!(file_meta("r.txt", "+1_024", "").expected_size, Some(1024));
}

#[test]
fn rejects_windows_paths_ads_devices_tail_dot_and_space() {
    for name in [
        "../a.txt",
        "a/b",
        "a\\b",
        "C:a",
        "a:stream",
        "a.",
        "a ",
        "CON",
        "con.txt",
        "CON .txt",
        "lpt9.bin",
        "COM¹.txt",
        "NUL",
        "CONOUT$",
        "a\0b",
        "a\nb",
        "a?",
        ".",
        "..",
    ] {
        assert_eq!(
            safe_name(name).unwrap_err().kind,
            ErrorKind::UnsafePath,
            "{name:?}"
        );
    }
    for name in [
        "report[1].txt",
        "two words.txt",
        "中文.pdf",
        "a..b.txt",
        "console.txt",
        "acom1.txt",
    ] {
        safe_name(name).unwrap();
    }
    for name in ["report.txt ", "report.txt.", "report.txt:stream"] {
        assert_eq!(
            parse_file_message(&input(&file_xml(name, "3", "")))
                .unwrap_err()
                .kind,
            ErrorKind::UnsafePath
        );
    }
}

#[test]
fn file_xml_remains_20k_and_record_xml_allows_500k() {
    let body = file_xml(&"a".repeat(22_000), "3", "");
    assert_eq!(
        parse_file_message(&input(&body)).unwrap_err().kind,
        ErrorKind::InvalidXml
    );
    let text = format!(
        "<dataitem datatype='1'><datadesc>{}</datadesc></dataitem>",
        "测".repeat(25_000)
    );
    let body = record_xml(&text);
    assert_eq!(
        parse_record_item(&input(&body), 0)
            .unwrap()
            .description
            .chars()
            .count(),
        25_000
    );
    let huge = record_xml(&format!(
        "<dataitem datatype='1'><datadesc>{}</datadesc></dataitem>",
        "x".repeat(500_001)
    ));
    assert_eq!(
        parse_record_item(&input(&huge), 0).unwrap_err().kind,
        ErrorKind::InvalidXml
    );
}

#[test]
fn rejects_dtd_entities_and_malformed_inner_xml() {
    for body in ["<!DOCTYPE msg><msg/>", "<!ENTITY x 'a'><msg/>", "<msg>"] {
        assert_eq!(
            parse_file_message(&input(body)).unwrap_err().kind,
            ErrorKind::InvalidXml
        );
    }
    let body = "<msg><appmsg><type>19</type><recorditem><![CDATA[<!DOCTYPE recordinfo><recordinfo/>]]></recorditem></appmsg></msg>";
    assert_eq!(
        parse_record_item(&input(body), 0).unwrap_err().kind,
        ErrorKind::InvalidXml
    );
}

#[test]
fn item_60_is_not_lost_to_history_truncation() {
    let items = (0..70)
        .map(|i| format!("<dataitem datatype='1'><datadesc>item {i}</datadesc></dataitem>"))
        .collect::<String>();
    let body = record_xml(&items);
    let parsed = parse_record_item(&input(&body), 60).unwrap();
    assert_eq!(parsed.item_count, Some(70));
    assert_eq!(parsed.item_index, Some(60));
    assert_eq!(parsed.description, "item 60");
}

#[test]
fn item_index_errors_and_not_loaded_are_distinct() {
    let body = record_xml(&item("1", "", "", ""));
    for index in [-1, 1, i64::MAX] {
        assert_eq!(
            parse_record_item(&input(&body), index).unwrap_err().kind,
            ErrorKind::InvalidIndex
        );
    }
    assert_eq!(
        parse_record_item(&input(&record_xml("")), 0)
            .unwrap_err()
            .kind,
        ErrorKind::NotLoaded
    );
    assert_eq!(
        parse_record_item(&input("<msg><appmsg><type>19</type></appmsg></msg>"), 0)
            .unwrap_err()
            .kind,
        ErrorKind::NotLoaded
    );
}

#[test]
fn metadata_only_never_expands_nested_record() {
    let body = record_xml("<dataitem datatype='17'><datatitle>nested</datatitle><recorditem><datalist><dataitem datatype='8'><datatitle>secret.txt</datatitle></dataitem></datalist></recorditem></dataitem>");
    let parsed = parse_record_item(&input(&body), 0).unwrap();
    assert_eq!(parsed.kind, Kind::MetadataOnly);
    assert_eq!(parsed.item_count, Some(1));
    assert_eq!(parsed.title, "nested");
    assert!(
        find_reference(Path::new("not-an-absolute-or-existing-path"), &parsed)
            .unwrap()
            .is_none()
    );
}

#[test]
fn text_and_all_metadata_types_require_no_filesystem() {
    for kind in [
        "1", "3", "6", "7", "17", "19", "22", "23", "29", "36", "37", "unknown",
    ] {
        let meta = record_meta(kind, "title", "0", "");
        assert!(matches!(meta.kind, Kind::Text | Kind::MetadataOnly));
        assert!(find_reference(Path::new("invalid-base"), &meta)
            .unwrap()
            .is_none());
    }
}

#[test]
fn finds_file_with_real_size_hash_and_live_readonly_handle() {
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), "msg/file/2026-09/report.txt", b"abc");
    let found = find_reference(root.path(), &file_meta("report.txt", "3", ABC_MD5))
        .unwrap()
        .unwrap();
    assert_eq!(found.path, path);
    assert_eq!(found.size, 3);
    assert_eq!(found.md5, ABC_MD5);
    assert_eq!(found.binding, Binding::Md5);
    assert!(found.warning.is_none());
    let mut bytes = Vec::new();
    let mut reader = found.file();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"abc");
    assert_eq!(serde_json::to_value(&found).unwrap()["size"], json!(3));
    drop(found);
    assert_eq!(fs::read(path).unwrap(), b"abc");
}

#[test]
fn no_hash_single_candidate_keeps_weak_binding() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "msg/file/report.txt", b"abc");
    let found = find_reference(root.path(), &file_meta("report.txt", "", ""))
        .unwrap()
        .unwrap();
    assert_eq!(found.binding, Binding::Heuristic);
    assert_eq!(found.md5, ABC_MD5);
    assert_eq!(found.size, 3);
    assert!(found.warning.unwrap().contains("弱绑定"));
}

#[test]
fn no_hash_multiple_candidates_are_ambiguous() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "msg/file/one/report.txt", b"abc");
    put(root.path(), "msg/file/two/report.txt", b"abc");
    assert_eq!(
        code(find_reference(
            root.path(),
            &file_meta("report.txt", "3", "")
        )),
        ErrorKind::Ambiguous
    );
}

#[test]
fn hash_selects_correct_content_not_mtime_or_first_month() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "msg/file/2026-09/report.txt", b"bad");
    let right = put(root.path(), "msg/file/2025-01/report (1).txt", b"abc");
    let found = find_reference(root.path(), &file_meta("report.txt", "3", ABC_MD5))
        .unwrap()
        .unwrap();
    assert_eq!(found.path, right);
}

#[test]
fn identical_hash_copies_are_explicit_and_deterministic() {
    let root = tempfile::tempdir().unwrap();
    let first = put(root.path(), "msg/file/a/report.txt", b"abc");
    put(root.path(), "msg/file/b/report.txt", b"abc");
    let found = find_reference(root.path(), &file_meta("report.txt", "3", ABC_MD5))
        .unwrap()
        .unwrap();
    assert_eq!(found.path, first);
    assert_eq!(found.equivalent_copies, 2);
}

#[test]
fn missing_size_mismatch_and_hash_mismatch_are_not_success() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        find_reference(root.path(), &file_meta("report.txt", "3", ""))
            .unwrap()
            .is_none()
    );
    put(root.path(), "msg/file/report.txt", b"bad");
    assert!(
        find_reference(root.path(), &file_meta("report.txt", "4", ""))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        code(find_reference(
            root.path(),
            &file_meta("report.txt", "3", ABC_MD5)
        )),
        ErrorKind::HashMismatch
    );
}

#[test]
fn file_search_does_not_accept_arbitrary_stem_prefix() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "msg/file/2026-09/report-secret.txt", b"abc");
    assert!(
        find_reference(root.path(), &file_meta("report.txt", "3", ""))
            .unwrap()
            .is_none()
    );
    for name in ["report.txt", "report(1).txt", "report (12).txt"] {
        assert!(file_match(name, "report.txt"));
    }
    for name in [
        "report-2.txt",
        "report ().txt",
        "report  (1).txt",
        "report(a).txt",
        "xreport.txt",
    ] {
        assert!(!file_match(name, "report.txt"));
    }
}

#[test]
fn filename_glob_characters_are_literal() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "msg/file/report1.txt", b"abc");
    let right = put(root.path(), "msg/file/report[1].txt", b"abc");
    let found = find_reference(root.path(), &file_meta("report[1].txt", "3", ""))
        .unwrap()
        .unwrap();
    assert_eq!(found.path, right);
}

#[test]
fn file_search_stays_in_msg_file() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "outside/report.txt", b"abc");
    put(root.path(), &rec_path("card", "F/0/report.txt"), b"abc");
    assert!(
        find_reference(root.path(), &file_meta("report.txt", "3", ""))
            .unwrap()
            .is_none()
    );
}

#[test]
fn rec_binary_types_use_only_their_own_subdirectory() {
    let root = tempfile::tempdir().unwrap();
    for (kind, sub) in [("4", "A"), ("5", "V"), ("8", "F")] {
        let path = put(
            root.path(),
            &rec_path("card", &format!("{sub}/0/report.txt")),
            b"abc",
        );
        let found = find_reference(root.path(), &record_meta(kind, "report.txt", "3", ""))
            .unwrap()
            .unwrap();
        assert_eq!(found.path, path);
    }
}

#[test]
fn rec_images_are_flat_not_index_directories_and_not_decrypted() {
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), &rec_path("card", "Img/0_t"), b"DAT");
    let found = find_reference(root.path(), &record_meta("2", "", "3", ""))
        .unwrap()
        .unwrap();
    assert_eq!(found.path, path);
    assert_eq!(found.md5, format!("{:x}", md5::compute(b"DAT")));
    assert_eq!(fs::read(path).unwrap(), b"DAT");
    assert!(!image_match("01_t", 0));
    assert!(image_match("0.jpg", 0));
}

#[test]
fn rec_same_index_across_cards_needs_hash() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), &rec_path("one", "F/0/report.txt"), b"bad");
    let correct = put(root.path(), &rec_path("two", "F/0/report.txt"), b"abc");
    assert_eq!(
        code(find_reference(
            root.path(),
            &record_meta("8", "report.txt", "3", "")
        )),
        ErrorKind::Ambiguous
    );
    let found = find_reference(root.path(), &record_meta("8", "report.txt", "3", ABC_MD5))
        .unwrap()
        .unwrap();
    assert_eq!(found.path, correct);
}

#[test]
fn rec_missing_title_size_only_is_explicit_and_bounded() {
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), &rec_path("card", "A/0/arbitrary.silk"), b"abc");
    let found = find_reference(root.path(), &record_meta("4", "", "3", ""))
        .unwrap()
        .unwrap();
    assert_eq!(found.path, path);
    drop(found);
    assert!(find_reference(root.path(), &record_meta("4", "", "", ""))
        .unwrap()
        .is_none());
    assert!(
        find_reference(root.path(), &record_meta("4", "different.silk", "3", ""))
            .unwrap()
            .is_none()
    );
}

#[test]
fn rec_other_username_and_other_index_are_not_candidates() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), &rec_path("card", "F/1/report.txt"), b"abc");
    put(
        root.path(),
        "msg/attach/other/2026-09/Rec/card/F/0/report.txt",
        b"abc",
    );
    assert!(
        find_reference(root.path(), &record_meta("8", "report.txt", "3", ""))
            .unwrap()
            .is_none()
    );
}

#[test]
fn record_item_above_50_finds_its_actual_binary() {
    let root = tempfile::tempdir().unwrap();
    let mut items = item("1", "", "", "").repeat(60);
    items.push_str(&item("8", "report.txt", "3", ABC_MD5));
    let metadata = parse_record_item(&input(&record_xml(&items)), 60).unwrap();
    let path = put(root.path(), &rec_path("card", "F/60/report.txt"), b"abc");
    put(root.path(), &rec_path("card", "F/0/report.txt"), b"bad");
    let found = find_reference(root.path(), &metadata).unwrap().unwrap();
    assert_eq!(found.path, path);
    assert_eq!(metadata.item_count, Some(61));
}

#[test]
fn record_image_variants_without_hash_are_ambiguous() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), &rec_path("card", "Img/0_t"), b"abc");
    put(root.path(), &rec_path("card", "Img/0.jpg"), b"bad");
    assert_eq!(
        code(find_reference(root.path(), &record_meta("2", "", "3", ""))),
        ErrorKind::Ambiguous
    );
    let found = find_reference(root.path(), &record_meta("2", "", "3", ABC_MD5))
        .unwrap()
        .unwrap();
    assert!(found.path.ends_with("0_t"));
}

#[test]
fn file_md5_is_outer_not_nested_appattach_field() {
    let body = "<msg><appmsg><type>6</type><title>report.txt</title><appattach><md5>invalid-nested-not-binding</md5><totallen>3</totallen></appattach></appmsg></msg>";
    assert_eq!(parse_file_message(&input(body)).unwrap().expected_md5, None);
}

#[test]
fn directory_limit_does_not_return_partial_success() {
    let root = tempfile::tempdir().unwrap();
    let mut scan = Scan::new(root.path()).unwrap();
    for i in 0..MAX_DIRECTORIES {
        let result = scan.pin_dir(&root.path().join(format!("missing-{i}")));
        if let Err(e) = result {
            assert_eq!(e.kind, ErrorKind::LimitExceeded);
            return;
        }
    }
    panic!("directory budget was not enforced");
}

#[test]
fn reference_lookup_preserves_all_synthetic_files_and_creates_nothing() {
    fn snapshot(
        root: &Path,
        current: &Path,
        out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                snapshot(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "msg/file/report.txt", b"abc");
    put(root.path(), &rec_path("card", "F/0/report.txt"), b"abc");
    put(root.path(), &rec_path("card", "Img/0_t"), b"DAT");
    let mut before = std::collections::BTreeMap::new();
    snapshot(root.path(), root.path(), &mut before);
    for metadata in [
        file_meta("report.txt", "3", ABC_MD5),
        record_meta("8", "report.txt", "3", ABC_MD5),
        record_meta("2", "", "3", ""),
    ] {
        let reference = find_reference(root.path(), &metadata).unwrap().unwrap();
        let encoded = serde_json::to_string(&reference).unwrap();
        assert!(!encoded.contains("api_key"));
    }
    let mut after = std::collections::BTreeMap::new();
    snapshot(root.path(), root.path(), &mut after);
    assert_eq!(before, after);
}

#[test]
fn candidate_directory_is_rejected_as_not_regular_file() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("msg/file/report.txt")).unwrap();
    assert_eq!(
        code(find_reference(
            root.path(),
            &file_meta("report.txt", "", "")
        )),
        ErrorKind::UnsafePath
    );
}

#[test]
fn relative_base_and_lexical_parent_or_dot_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    for path in [
        PathBuf::from("relative"),
        root.path().join(".."),
        root.path().join(".").join("child"),
    ] {
        assert_eq!(
            code(find_reference(&path, &file_meta("report.txt", "", ""))),
            ErrorKind::UnsafePath
        );
    }
}

#[test]
fn scanning_depth_is_bounded_even_without_candidates() {
    let root = tempfile::tempdir().unwrap();
    let path = format!("msg/file/{}/unrelated", vec!["d"; MAX_DEPTH + 2].join("/"));
    put(root.path(), &path, b"x");
    assert_eq!(
        code(find_reference(
            root.path(),
            &file_meta("report.txt", "", "")
        )),
        ErrorKind::LimitExceeded
    );
}

#[test]
fn directory_entry_budget_is_consumed_and_not_truncated() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "a", b"a");
    put(root.path(), "b", b"b");
    assert_eq!(
        read_entries(root.path(), &mut 1).err().unwrap().kind,
        ErrorKind::LimitExceeded
    );
}

#[test]
fn candidate_limit_fails_instead_of_choosing_first() {
    let root = tempfile::tempdir().unwrap();
    for i in 0..=MAX_CANDIDATES {
        put(root.path(), &format!("msg/file/report ({i}).txt"), b"abc");
    }
    assert_eq!(
        code(find_reference(
            root.path(),
            &file_meta("report.txt", "3", ABC_MD5)
        )),
        ErrorKind::LimitExceeded
    );
}

#[test]
fn hash_budget_is_cumulative_and_growth_cannot_bypass_it() {
    assert_eq!(MAX_HASH_BYTES, 524_288_000);
    let mut left = 5;
    let (hash, size) = md5_reader(&mut Cursor::new(b"abc"), &mut left).unwrap();
    assert_eq!((hash.as_str(), size, left), (ABC_MD5, 3, 2));
    assert_eq!(
        md5_reader(&mut Cursor::new(b"abc"), &mut left)
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
    // 读到上限后还有数据，相当于 stat 后文件增长；探针必须明确失败。
    let mut growing = std::io::repeat(0).take(9);
    assert_eq!(
        md5_reader(&mut growing, &mut 8).unwrap_err().kind,
        ErrorKind::LimitExceeded
    );
    let (_, size) = md5_reader(&mut Cursor::new(b"abc"), &mut 3).unwrap();
    assert_eq!(size, 3);
}

#[test]
fn oversized_file_is_rejected_before_reading_bytes() {
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), "msg/file/report.txt", b"");
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(MAX_HASH_BYTES + 1)
        .unwrap();
    assert_eq!(
        code(find_reference(
            root.path(),
            &file_meta("report.txt", "", "")
        )),
        ErrorKind::LimitExceeded
    );
}

#[test]
fn directory_mutation_between_scans_is_detected() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "one", b"a");
    let mut scan = Scan::new(root.path()).unwrap();
    scan.entries(root.path()).unwrap();
    fs::write(root.path().join("two"), b"b").unwrap();
    assert_eq!(scan.verify().unwrap_err().kind, ErrorKind::Changed);
}

#[test]
fn candidate_growth_before_open_is_detected() {
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), "report.txt", b"abc");
    let entry = read_entries(root.path(), &mut 10).unwrap().pop().unwrap();
    fs::write(&path, b"abcdef").unwrap();
    assert_eq!(
        candidate(
            &path,
            &entry,
            &file_meta("report.txt", "", ""),
            &mut Vec::new()
        )
        .unwrap_err()
        .kind,
        ErrorKind::Changed
    );
}

#[test]
fn previously_missing_record_subdir_cannot_appear_silently() {
    let root = tempfile::tempdir().unwrap();
    let mut scan = Scan::new(root.path()).unwrap();
    let path = root.path().join("Rec");
    assert!(!scan.pin_dir(&path).unwrap());
    fs::create_dir(&path).unwrap();
    assert_eq!(scan.verify().unwrap_err().kind, ErrorKind::Changed);
}

#[cfg(windows)]
#[test]
fn live_file_reference_blocks_write_delete_and_ancestor_rename() {
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), "msg/file/report.txt", b"abc");
    let found = find_reference(root.path(), &file_meta("report.txt", "3", ABC_MD5))
        .unwrap()
        .unwrap();
    assert!(fs::write(&path, b"bad").is_err());
    assert!(fs::remove_file(&path).is_err());
    assert!(fs::rename(root.path().join("msg"), root.path().join("moved")).is_err());
    let mut writable = &found.file.file;
    use std::io::Write;
    assert!(writable.write_all(b"bad").is_err());
    drop(found);
    assert_eq!(fs::read(path).unwrap(), b"abc");
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("cmd.exe");
    command
        .args(["/d", "/u", "/c", "mklink", "/J"])
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(target.to_string_lossy().replace('/', "\\"))
        .creation_flags(0x08000000);
    eprintln!("synthetic junction command: {command:?}");
    let output = command.output().unwrap();
    for bytes in [&output.stdout, &output.stderr] {
        let words: Vec<_> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        eprint!("{}", String::from_utf16_lossy(&words));
    }
    assert!(output.status.success());
}

#[cfg(windows)]
#[test]
fn rejects_ancestor_junction_before_normalizing_base() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    put(outside.path(), "account/msg/file/report.txt", b"abc");
    let link = root.path().join("junction");
    junction(&link, outside.path());
    let result = find_reference(&link.join("account"), &file_meta("report.txt", "3", ""));
    fs::remove_dir(&link).unwrap();
    assert_eq!(code(result), ErrorKind::UnsafePath);
}

#[cfg(windows)]
#[test]
fn rejects_junction_inside_file_candidate_tree() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    put(outside.path(), "report.txt", b"abc");
    fs::create_dir_all(root.path().join("msg/file")).unwrap();
    let link = root.path().join("msg/file/junction");
    junction(&link, outside.path());
    let result = find_reference(root.path(), &file_meta("report.txt", "3", ""));
    fs::remove_dir(&link).unwrap();
    assert_eq!(code(result), ErrorKind::UnsafePath);
}

#[cfg(windows)]
#[test]
fn rejects_rec_subdir_junction() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    put(outside.path(), "0/report.txt", b"abc");
    let link = root.path().join(rec_path("card", "F"));
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    junction(&link, outside.path());
    let result = find_reference(root.path(), &record_meta("8", "report.txt", "3", ""));
    fs::remove_dir(&link).unwrap();
    assert_eq!(code(result), ErrorKind::UnsafePath);
}

#[cfg(windows)]
#[test]
fn rejects_network_device_and_ads_base_paths() {
    for path in [
        r"\\server\share\account",
        r"\\.\C:\account",
        r"C:\account:stream",
        r"C:\NUL\account",
        r"C:\account.\child",
        r"C:\account \child",
    ] {
        assert_eq!(
            code(find_reference(
                Path::new(path),
                &file_meta("report.txt", "", "")
            )),
            ErrorKind::UnsafePath,
            "{path}"
        );
    }
}
