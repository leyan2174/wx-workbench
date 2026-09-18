use super::*;
use crate::cli::{Cli, Commands};
use clap::{CommandFactory, Parser};
use serde_json::{json, Value};

fn parse(argv: &[&str]) -> Command {
    let Commands::Queries(command) = Cli::try_parse_from(argv).unwrap().command else {
        panic!("expected a read-only detail query");
    };
    command
}

fn request(argv: &[&str]) -> Value {
    serde_json::to_value(parse(argv).into_request().unwrap()).unwrap()
}

#[test]
fn detail_commands_are_flat_top_level_commands() {
    // Match the existing full command-tree test's stack allowance.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let tree = Cli::command();
            tree.clone().debug_assert();
            for name in [
                "tags",
                "tag-members",
                "decode-refer",
                "decode-file-message",
                "decode-record-item",
                "voice-messages",
            ] {
                assert!(tree.find_subcommand(name).is_some(), "{name}");
            }
            assert!(tree.find_subcommand("queries").is_none());
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn tags_map_to_existing_requests_without_losing_the_target() {
    assert_eq!(request(&["wx", "tags"]), json!({"cmd":"contact_tags"}));
    assert_eq!(
        request(&["wx", "tag-members", " Friends "]),
        json!({"cmd":"tag_members", "tag_name":" Friends "})
    );
}

#[test]
fn message_queries_preserve_identity_and_default_timestamp() {
    for (cli_name, wire_name) in [
        ("decode-refer", "decode_refer"),
        ("decode-file-message", "decode_file_message"),
    ] {
        assert_eq!(
            request(&["wx", cli_name, "peer", "7"]),
            json!({"cmd":wire_name,"chat":"peer","local_id":7,"create_time":0})
        );
        assert_eq!(
            request(&["wx", cli_name, "peer", "7", "1700000000", "--json"]),
            json!({"cmd":wire_name,"chat":"peer","local_id":7,"create_time":1700000000})
        );
        assert_eq!(
            request(&["wx", cli_name, "peer", "7", "-1"])["create_time"],
            -1
        );
    }
    assert_eq!(
        request(&["wx", "decode-record-item", "peer", "7", "0"]),
        json!({"cmd":"decode_record_item","chat":"peer","local_id":7,
            "item_index":0,"create_time":0})
    );
    assert_eq!(
        request(&["wx", "decode-record-item", "peer", "7", "3", "1700000000",]),
        json!({"cmd":"decode_record_item","chat":"peer","local_id":7,
            "item_index":3,"create_time":1700000000})
    );
}

#[test]
fn voice_defaults_and_pagination_match_the_read_only_contract() {
    assert_eq!(
        request(&["wx", "voice-messages", "peer"]),
        json!({"cmd":"voice_messages","chat":"peer","limit":20,"offset":0})
    );
    for flag in ["-n", "--limit"] {
        assert_eq!(
            request(&[
                "wx",
                "voice-messages",
                "peer",
                flag,
                "500",
                "--offset",
                "1000000",
            ]),
            json!({"cmd":"voice_messages","chat":"peer","limit":500,"offset":1000000})
        );
    }
    assert_eq!(
        request(&["wx", "voice-messages", "peer", "-n", "1"])["limit"],
        1
    );
}

#[test]
fn voice_dates_reuse_cli_local_time_and_inclusive_day_end() {
    let value = request(&[
        "wx",
        "voice-messages",
        "peer",
        "--since",
        "2024-01-15",
        "--until",
        "2024-01-15",
    ]);
    assert_eq!(
        value["since"],
        time::parse_time("2024-01-15 00:00:00").unwrap()
    );
    assert_eq!(
        value["until"],
        time::parse_time("2024-01-15 23:59:59").unwrap()
    );
    for date in ["2024-01-15 12:34", "2024-01-15 12:34:56"] {
        let value = request(&[
            "wx",
            "voice-messages",
            "peer",
            "--since",
            date,
            "--until",
            date,
        ]);
        assert_eq!(value["since"], time::parse_time(date).unwrap());
        assert_eq!(value["since"], value["until"]);
    }
    assert!(parse(&[
        "wx",
        "voice-messages",
        "peer",
        "--since",
        "2024-01-16",
        "--until",
        "2024-01-15",
    ])
    .into_request()
    .is_err());
}

#[test]
fn json_is_detected_for_every_new_command() {
    for argv in [
        vec!["wx", "tags"],
        vec!["wx", "tag-members", "Friends"],
        vec!["wx", "decode-refer", "peer", "1"],
        vec!["wx", "decode-file-message", "peer", "1"],
        vec!["wx", "decode-record-item", "peer", "1", "0"],
        vec!["wx", "voice-messages", "peer"],
    ] {
        assert!(!parse(&argv).json());
        let mut with_json = argv;
        with_json.push("--json");
        assert!(parse(&with_json).json());
    }
}

#[test]
fn invalid_targets_numbers_dates_and_write_options_are_rejected() {
    for argv in [
        vec!["wx", "tags", "unexpected"],
        vec!["wx", "tag-members"],
        vec!["wx", "tag-members", ""],
        vec!["wx", "tag-members", " \t"],
        vec!["wx", "voice-messages", ""],
        vec!["wx", "decode-refer", " ", "1"],
        vec!["wx", "decode-file-message", "", "1"],
        vec!["wx", "decode-record-item", "", "1", "0"],
        vec!["wx", "decode-record-item", "peer", "1"],
        vec!["wx", "decode-record-item", "peer", "1", "-1"],
        vec!["wx", "decode-record-item", "peer", "1", "1.5"],
        vec![
            "wx",
            "decode-record-item",
            "peer",
            "1",
            "9223372036854775808",
        ],
        vec!["wx", "voice-messages", "peer", "--limit", "0"],
        vec!["wx", "voice-messages", "peer", "--limit", "501"],
        vec!["wx", "voice-messages", "peer", "--limit", "1.5"],
        vec!["wx", "voice-messages", "peer", "--offset", "-1"],
        vec!["wx", "voice-messages", "peer", "--offset", "1000001"],
        vec![
            "wx",
            "voice-messages",
            "peer",
            "--offset",
            "18446744073709551616",
        ],
        vec!["wx", "voice-messages", "peer", "--since", "2024-02-30"],
        vec!["wx", "voice-messages", "peer", "--until", "1700000000"],
        vec!["wx", "voice-messages", "peer", "--output", "voices"],
        vec!["wx", "voice-messages", "peer", "--transcribe"],
        vec!["wx", "voice-messages", "peer", "--format", "mp3"],
        vec!["wx", "decode-file-message", "peer", "1", "--output", "file"],
        vec!["wx", "decode-record-item", "peer", "1", "0", "--download"],
    ] {
        assert!(Cli::try_parse_from(&argv).is_err(), "{argv:?}");
    }
    for name in ["decode-refer", "decode-file-message", "decode-record-item"] {
        for id in ["0", "-1", "1.5", "9223372036854775808"] {
            let mut argv = vec!["wx", name, "peer", id];
            if name == "decode-record-item" {
                argv.push("0");
            }
            assert!(Cli::try_parse_from(&argv).is_err(), "{argv:?}");
        }
    }
    let oversized = "x".repeat(4097);
    assert!(Cli::try_parse_from(["wx", "tag-members", &oversized]).is_err());
    assert!(Cli::try_parse_from(["wx", "voice-messages", &oversized]).is_err());
}
