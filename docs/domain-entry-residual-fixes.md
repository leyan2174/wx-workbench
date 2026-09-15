# Domain entry residual fixes

These changes reconcile three still-present production findings from
`phase5-domain-audit-snapshot.md`. They do not replace the earlier baseline's
test evidence or claim new execution results. No Cargo commands, commits or
real-account access were performed. Test execution: pending parent validation.

## Contacts

`adapters::wechat::contacts::source_keys` owns the ordered cache source descriptor.
`daemon::query::mcp_contacts::source` and
`daemon::query::mcp_contacts_legacy::q_contacts_legacy` use those keys instead of
embedding WeChat database paths. The host still resolves through its fixed
DbCache; it does not discover another account. Primary absence permits the same
backslash-key fallback as before; primary errors do not fall back. Contact/tag
selection, output projection and error text are unchanged.

`mcp_contacts_legacy/tests.rs` adds
`adapter_descriptors_keep_legacy_contacts_and_tags_bound_to_both_cache_key_spellings`.
It uses the existing SQLCipher fixture helpers and real DbCache against two
synthetic accounts, one key spelling each, and checks both legacy contacts and
tag queries, display-name overrides, repeat reads and unchanged encrypted files.
`primary_contact_source_error_does_not_fall_back_to_a_valid_alias` supplies a wrong
primary key alongside a valid compatibility key and requires all three reads to
fail rather than hide the primary error.
The standalone contact/read-only fixtures continue to include the real contact
adapter and therefore obtain this descriptor without a new stub.

## Favorites

CLI selectors now resolve to the existing business FavoriteKind. The narrow
`service::favorite_filter::legacy_wire_type` compatibility owner maps these to
the unchanged wire numbers (1/2/5/19/20); CLI no longer owns WeChat codes.
The existing adapter still owns numeric storage filtering/row interpretation.
The command's allowed lowercase labels and request fields are unchanged;
absent optional fields remain omitted. Invalid direct calls fail before
transport rather than silently removing the requested filter.

The CLI requires an array of object items, boolean has_more and a count matching
the array. Missing/malformed fields are errors, never default empty success.
has_more=true emits a continuation notice to stderr explaining that the limit
can be raised or the filter narrowed; no nonexistent offset/cursor command is
promised. JSON/YAML stdout remains the previous items array, with the same item
fields, and daemon JSON retains items/count/has_more unchanged. A valid empty
page with has_more=true is distinct from an exhausted empty result.

New tests in `src/cli/favorites/tests.rs` cover semantic labels and exact request
JSON, malformed responses, unchanged item serialization and continuation notices.
`service::favorite_filter` tests every known projection and the unmappable Other
kind. The existing real-account-cache synthetic test in
`daemon/query/favorites_source_tests.rs` additionally checks limit=0 with more
matches versus a truly empty/exhausted query and an exactly full final page.

## Emoticon batch outcome

`daemon::operations::export_emoticons::export` still runs the entire business
export_batch and prints the existing successful/failed counts and output path.
After that, `finish_report` maps the typed counts through the existing
`ipc::outcome::BusinessOutcome::from_counts`: all successful (including an empty
selection) succeeds, mixed results return BusinessFailure(Partial), and all
failed return BusinessFailure(Failure). Successful files and per-item reports
are not removed on failure. Dry-run preview remains non-downloading success.

This deliberately changes the old misleading zero exit on failed downloads.
It reuses the existing operation-worker finish path: partial exit 20, ordinary
failure exit 1. The current task worker interprets a non-success worker outcome
as status=failed and retains its exit code and safe partial/failure message;
this patch does not invent a new task status or add an emoticon task kind.
The export is currently a ToolkitOperation, so no newly reachable persistent
emoticon task route is claimed.

`partial_batch_preserves_artifacts_continues_and_returns_shared_failure` in
`daemon/operations/export_emoticons.rs` drives the real business batch with a
synthetic file exporter: the second item fails, the third still writes, both
successful artifacts remain, and the host returns the shared partial outcome.
It also covers all-failed and all-successful batches. This is not a network
download integration test. The already-existing
`daemon::operation_worker::outcome_tests::actual_worker_exit_codes_distinguish_partial_and_refusal_without_details`
covers the shared real worker exit adapter and should be included in parent
validation, without this document treating its previous presence as a new pass.

## Parent validation

Run the root check/tests after all agents finish. Relevant cases are the new
contact descriptor test, CLI favorites tests, favorite compatibility tests,
favorites_source_tests, the emoticon host test and existing worker outcome tests.
Re-run contact-rows and mcp-readonly-security standalone fixtures as appropriate.
All results for this patch remain pending parent validation; no total-test count
or pass claim is inferred from source inspection or formatting.
