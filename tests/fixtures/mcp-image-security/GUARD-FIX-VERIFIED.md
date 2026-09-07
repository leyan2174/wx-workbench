# Independent Guard Fix Verification

> Historical verification snapshot. The aggregate below belongs to its named
> log and source version; the documentation cleanup did not run this audit or
> replace its numbers with a later main-project result.

Final complete run: 22 tests, 20 passed, 0 failed, 2 ignored, exit 0.
Evidence: full-guard-fix-final.log. This is a fresh aggregate run, not a sum of
earlier partial results. Fixture dependency/lint warnings remain in the log.

- The original directory-swap security assertion is unchanged and now passes:
  protected directory identity is preserved, export rejects, published=false.
- Production code moves the original HostOutputGuard through the internal
  image query into spawn_blocking, and verifies that same guard immediately
  before persist_noclobber. It does not trust a newly opened same-path guard.
- An audit-only AST build instruments the production image implementation
  immediately after staged sync_all. Replacing a protected directory there
  returns path identity changed and produces no final image. Normal export works.
- Actual voice publish_wav_noclobber rejects a protected-directory identity
  replacement inside before_commit, before final WAV publication.
- With the staged file still open, output-directory rename attempts are denied
  by Windows (OS5). Tests verify the original output identity and protected
  sentinel remain unchanged; publication remains in that approved directory.
- Real cold/warm/redecrypt cache first exports and mtime persistence still pass.

Two actual symlink tests remain ignored because creation needs unavailable
Windows privileges (OS1314). Directory junction tests pass. No atomic guarantee
is claimed for the final verification-to-filesystem-commit interval.

No production edits were made by this auditor. Original safe21 evidence and
historical failing logs remain. No running audit session remains.

Published-output/response-delivery failure is retained as the documented
limitation, with actual IPC reader and MCP serialization evidence. Voice host
ID/budget orchestration is not claimed tested here: only the real voice publisher
and actual WAV validator are exercised; the image-only host probe's unrelated
voice orchestration boundary deliberately panics if invoked.
