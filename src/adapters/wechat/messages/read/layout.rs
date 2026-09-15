//! WeChat message table names; wire catalogs require canonical spelling.
pub fn username_hash(username: &str) -> String {
    format!("{:x}", md5::compute(username.as_bytes()))
}

pub fn table_for_username(username: &str) -> String {
    format!("Msg_{}", username_hash(username))
}

/// Syntax and ownership only; this does not authenticate an account or prove uniqueness.
#[expect(
    clippy::too_many_arguments,
    reason = "validate the existing physical proof without duplicating its storage type"
)]
pub fn valid_voice_source(
    username: &str,
    message_source: &str,
    media_source: &str,
    table: &str,
    local_id: i64,
    server_id: i64,
    max_source_bytes: Option<usize>,
) -> bool {
    let source = |s: &str, prefix: &str| {
        max_source_bytes.is_none_or(|max| s.len() <= max)
            && s.strip_prefix(prefix)
                .and_then(|v| v.strip_suffix(".db"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    !username.is_empty()
        && username.len() <= 1024
        && !username.chars().any(char::is_control)
        && source(message_source, "message/message_")
        && source(media_source, "message/media_")
        && table == table_for_username(username)
        && local_id > 0
        && server_id != 0
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 32 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn canonical_table_hash(table: &str) -> Option<&str> {
    table
        .strip_prefix("Msg_")
        .filter(|hash| valid_hash(hash) && !hash.bytes().any(|byte| byte.is_ascii_uppercase()))
}

pub(super) fn valid_sqlite_table(table: &str) -> bool {
    table
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Msg_"))
        && table.get(4..).is_some_and(valid_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_source_profiles_preserve_strict_ownership_and_limits() {
        let table = table_for_username("peer");
        let valid = |message: &str, media: &str, table: &str, max| {
            valid_voice_source("peer", message, media, table, 1, -1, max)
        };
        assert!(valid(
            "message/message_00.db",
            "message/media_0.db",
            &table,
            Some(128)
        ));
        for source in [
            "message/message_.db",
            "message/message_+1.db",
            "message/message_1_0.db",
            "message/../message_1.db",
            "message/message_1.db/extra",
            "message\\message_1.db",
        ] {
            assert!(!valid(source, "message/media_0.db", &table, None));
        }
        assert!(!valid(
            "message/message_0.db",
            "message/message_0.db",
            &table,
            None
        ));
        assert!(!valid(
            "message/message_0.db",
            "message/media_0.db",
            &table_for_username("other"),
            None
        ));
        let at_limit = format!("message/message_{}.db", "0".repeat(109));
        assert_eq!(at_limit.len(), 128);
        assert!(valid(&at_limit, "message/media_0.db", &table, Some(128)));
        let over_limit = format!("message/message_{}.db", "0".repeat(110));
        assert!(valid(&over_limit, "message/media_0.db", &table, None));
        assert!(!valid(&over_limit, "message/media_0.db", &table, Some(128)));
    }
    #[test]
    fn canonical_wire_and_sqlite_case_profiles_share_hash_rules() {
        let table = table_for_username("exact username");
        assert_eq!(
            canonical_table_hash(&table),
            Some(username_hash("exact username").as_str())
        );
        assert!(valid_sqlite_table(&table.to_ascii_uppercase()));
        assert!(canonical_table_hash(&table.to_ascii_uppercase()).is_none());
        assert!(canonical_table_hash("Msg_not_a_hash").is_none());
        assert_ne!(
            table_for_username("exact username"),
            table_for_username("exact username ")
        );
    }
}
