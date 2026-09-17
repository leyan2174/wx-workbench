# Domain audit snapshot

> 阶段记录：本文保留实施当时的路径、限制和测试结果，不是当前接口规范。当前职责与入口以[架构说明](architecture.md)和[文档索引](README.md)为准。

This records the read-only audit at baseline `5115419`, not a fresh validation of
later parallel changes. Positive test evidence below was read in the existing
`C:/CodexLocal/wx-cli-final-root-tests-2.log` and the fixture evidence recorded in
`docs/architecture-phase5-progress.md`. No tests were run by this auditor.
These findings must be reconciled with subsequent owners' changes in the final
requirements matrix; absence of search matches is not completion evidence.

| Domain | Positive implementation and test evidence | Confirmed residual or intentional difference |
| --- | --- | --- |
| Contacts / groups / tags | `business/contacts.rs` and `adapters/wechat/contacts::SqliteContacts` feed real contact/member queries. `member_schema_variants_and_empty_complete_groups_are_detected`, `broken_member_links_and_duplicate_room_identity_do_not_fall_back`, and `observed_senders_are_explicit_and_unresolved_sender_is_an_error` passed. Same-display-name member fixtures retain distinct username IDs. Label fixtures reject malformed associations and test account-cache isolation. | `daemon/query/mcp_contacts.rs::source` and `mcp_contacts_legacy::q_contacts_legacy` still held source path literals. Observed senders are explicitly incomplete membership, not a complete-group substitute. |
| Moments | `business/moments.rs` and `adapters/wechat/moments::Timeline::scan` feed query and export. `sqlite_fallback_conflict_recovery_and_schema_are_shared` and `query_and_export_make_author_and_invalid_content_policies_explicit` passed. Recorded author wins conflicts; embedded author is fallback. Local-cache scope is explicit. | Production counterexample: `daemon/operations/sns_timeline::export_for` -> `build_cache_index` -> `export_database_with_publication` -> `recover_post_media`; `toolkit/sns/cache.rs` still interpreted DAT/layout/media recovery. Missing database returns success, explicitly tested by `missing_sns_db_is_legacy_success_without_creating_output`. Query Effective and export RecordedCompatibility author policies are intentional differences, not one interchangeable mode. |
| Favorites | `daemon/query.rs::q_favorites` uses typed business listing and the favorites adapter. `favorite_query_uses_real_account_caches_and_preserves_public_projection` passed alongside duplicate-identity and malformed-schema rejection. | `cli/favorites::parse_fav_type` retained numeric format codes. The CLI discarded `has_more` and defaulted missing items to an empty array, losing completeness information. |
| Articles | `q_biz_articles` uses business listing and a complete OfficialPush snapshot. `sqlite_shared_snapshot_decodes_all_shards_without_sessions_or_local_id_identity` and `sqlite_unmapped_stream_is_partial_but_bad_content_is_failure` passed. | No confirmed counterexample in this audit. Equal URLs are not identity; missing stream mapping is partial, malformed content fails. Sessions are used only for explicitly requested unread membership. |
| Emoticons | Adapter catalog -> business select/export_batch -> `daemon/operations/export_emoticons.rs::export`. `batch_keeps_partial_failure_and_continues_in_order` and `catalog_reference_and_legacy_cache_results_are_not_hash_proofs` passed. | Production counterexample: host export returned Ok even when batch contained failures, losing execution failure semantics despite correct typed partial results. Invalid supplied catalog must not fall back; catalog-free LegacyCache is deliberately not a hash proof. |

For the later strict image/attachment slice, see
`strict-media-host-boundary.md`. Do not attribute those later changes or their
unrun tests to the positive baseline evidence above.
