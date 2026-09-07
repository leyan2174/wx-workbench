# Scalar Metadata Fix Verification

The malformed scalar constraint finding in REVIEW.md is fixed in
src/toolkit/attachment_refs.rs. The original report and failing-run logs
remain historical evidence, not the current test result.

Only md5, totallen, fullmd5 and datasize use the new scalar reader.
Child elements fail with InvalidMetadata. All text/CDATA segments are
read, including text separated by comments, before hash/number validation.
Missing and empty fields retain their existing behavior. Rich descriptions
and the outer-only file MD5 selection contract are unchanged.

Verification:

- Original security assertions unchanged: 21 passed, 0 failed, 0 ignored.
- This includes the original five failures and the real SQL query boundary.
- SQL malformed nested hash now returns exit_code 1 and InvalidMetadata,
  with no successful heuristic attachment reference.
- Main attachment regression: 50 passed, 0 failed, 0 ignored (47 existing
  tests plus 3 new table-driven compatibility/structure tests).
- Other main-project tests were filtered out, not executed.
- Tests use synthetic fixtures; no real account data or daemon operations.

Evidence files in this directory:

- scalar-fix.diff: production source delta against the pre-fix snapshot.
- scalar-fix-test.log: complete security test stdout/stderr.
- scalar-fix-main-test.log: complete main regression stdout/stderr.
- attachment_refs.before-scalar-fix.txt: pre-fix production source.

The security log contains an encoding artifact in the Windows mklink
success message; the corresponding junction test passed.
