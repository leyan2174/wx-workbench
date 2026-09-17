use super::output::{print_value, resolve};
use anyhow::Result;
use serde::Serialize;

const NATIVE_COMMANDS: [&str; 25] = [
    "setup",
    "cleanup",
    "status",
    "progress",
    "database decrypt",
    "chats export-all",
    "emoticons export",
    "chats export-delta",
    "chats plan",
    "media video decode",
    "media image decode",
    "media image decode-cache",
    "media image decode-directory",
    "chats export",
    "moments export-snapshot",
    "moments export",
    "chats export-messages",
    "moments archive",
    "keys image",
    "keys database",
    "keys watch-image",
    "monitor",
    "latency",
    "web",
    "gui",
];

#[derive(Serialize)]
struct Capabilities {
    native_commands: Vec<&'static str>,
    implementation: &'static str,
}

pub(super) fn execute(json: bool) -> Result<()> {
    let status = Capabilities {
        native_commands: NATIVE_COMMANDS.to_vec(),
        implementation: "native-rust",
    };
    print_value(&serde_json::to_value(status)?, &resolve(json))
}

#[test]
fn capabilities_list_unique_formal_entry_points() {
    let unique: std::collections::HashSet<_> = NATIVE_COMMANDS.iter().collect();
    assert_eq!(unique.len(), 25);
    for command in NATIVE_COMMANDS {
        assert!(!command.contains("toolkit"));
        assert!(!command.contains("native"));
        assert!(
            command.contains(' ')
                || matches!(
                    command,
                    "setup"
                        | "cleanup"
                        | "status"
                        | "progress"
                        | "monitor"
                        | "latency"
                        | "web"
                        | "gui"
                )
        );
    }
}
