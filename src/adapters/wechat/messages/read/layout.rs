//! WeChat message table names; wire catalogs require canonical spelling.
pub fn username_hash(username: &str) -> String {
    format!("{:x}", md5::compute(username.as_bytes()))
}

pub fn table_for_username(username: &str) -> String {
    format!("Msg_{}", username_hash(username))
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
