"""历史手工图生成器；原版已归档，不再允许覆盖当前 Mermaid 图件。"""
if __name__ == "__main__":
    raise SystemExit("Historical renderer disabled. Use node docs/diagrams/export_png.cjs <playwright-module> <local-mermaid-bundle>; see docs/diagrams/README.md.")
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from datetime import datetime, timezone
from html import escape

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SOURCES = [
    "docs/rust-migration.md", "docs/legacy-workflow-gap-audit.md",
    "docs/architecture.md", "src/main.rs", "src/cli/mod.rs", "src/cli/toolkit.rs",
    "src/toolkit/mod.rs", "src/toolkit/sns/mod.rs", "src/cli/export_sns.rs",
    "src/toolkit/sns/export.rs", "src/toolkit/sns/cache.rs", "src/cli/export_chats.rs",
    "src/cli/sns_timeline.rs",
    "src/daemon/query/export.rs", "src/toolkit/contact_metadata.rs",
    "src/toolkit/chat_merge.rs", "src/toolkit/chat_delta.rs", "src/mcp/protocol.rs",
    "src/mcp/mod.rs", "src/mcp/PROTOCOL.md", "src/cli/mcp.rs", "src/cli/transport.rs",
    "src/cli/export_delta.rs", "src/daemon/query/export_delta.rs", "src/daemon/query.rs",
    "src/cli/chat_plan.rs", "src/toolkit/chat_plan.rs", "src/toolkit/chat_plan_tests.rs",
    "src/cli/asr_database.rs", "src/toolkit/asr/database_media.rs",
    "src/toolkit/asr/mod.rs", "src/toolkit/asr/local.rs", "src/toolkit/asr/openai.rs",
    "src/toolkit/asr/writeback.rs", "src/cli/asr.rs", "src/toolkit/sns/video_runtime.rs",
    "src/toolkit/legacy.rs", "src/toolkit/audio/mod.rs", "src/toolkit/audio/batch.rs",
    "src/runtime.rs", "src/daemon/server.rs", "src/ipc.rs", "src/cli/sns_video.rs", "Cargo.toml",
    "tests/chat_plan_runtime.rs", "tests/delta_plan_security.rs",
    "src/daemon/query/strict_message.rs", "src/daemon/query/mcp_attachments.rs",
    "src/daemon/query/mcp_refer.rs", "src/daemon/query/mcp_contacts.rs", "src/daemon/query/mcp_voice.rs",
    "src/toolkit/attachment_refs.rs", "src/attachment/mod.rs",
    "src/toolkit/asr/cache.rs", "src/toolkit/asr/cached.rs",
    "src/cli/history.rs", "src/daemon/query/history_selection.rs",
    "tests/mcp_readonly_runtime.rs", "tests/fixtures/mcp-readonly-runtime/attachments.rs",
    "docs/native-attachment-contract.md",
    "src/daemon/query/mcp_image.rs", "src/attachment/native_image.rs", "src/daemon/cache.rs",
    "src/cli/mcp_voice.rs", "src/daemon/query/mcp_audio.rs", "src/attachment/local_files.rs",
    "src/toolkit/asr/prepared_audio.rs", "src/toolkit/audio/publish.rs", "README.md",
    "src/toolkit/asr/receipt.rs", "src/attachment/image_metadata.rs",
    "src/toolkit/run_status.rs", "src/toolkit/run_status_tests.rs", "tests/run_status_runtime.rs",
    "src/cli/export_emoticons.rs", "src/cli/toolkit_run_prepare.rs", "tests/emoticons_runtime.rs",
    "src/toolkit/emoticons/catalog.rs", "src/toolkit/emoticons/download.rs",
    "src/toolkit/emoticons/download_tests.rs",
    "src/toolkit/sns/album_images.rs", "src/toolkit/sns/download.rs", "src/cli/sns_album.rs",
    "src/toolkit/sns/album.rs", "src/toolkit/sns/album_videos.rs",
    "src/toolkit/sns/album_render.rs", "src/toolkit/sns/publish.rs",
    "src/toolkit/sns/album_images_tests.rs", "src/toolkit/sns/album_videos_tests.rs",
    "src/toolkit/sns/album_render_tests.rs", "src/toolkit/sns/publish_tests.rs",
    "tests/sns_album_runtime.rs", "tests/fixtures/sns-album-render/oracle.py",
    "docs/diagrams/render_architecture.py", "docs/diagrams/export_png.cjs", "docs/diagrams/README.md",
]
SNAPSHOT = "2026-09-07 / SNS album: 1121 executions passed, 0 failed, 11 ignored / MSVC 10 warnings"
MCP_TOOLS = ["get_recent_sessions", "get_contacts", "get_chat_history", "search_messages",
             "decode_transfer", "decode_location", "get_new_messages", "get_chat_images",
             "get_contact_tags", "get_tag_members", "decode_refer", "get_voice_messages",
             "decode_file_message", "decode_record_item", "decode_image", "decode_voice", "transcribe_voice"]
# 这些是人工核对过的源码锚点，不把简单文本匹配冒充 Rust 调用图解析。
WIRING = [
    ("sns-album-images-foundation", "src/toolkit/sns/mod.rs", "pub(crate) mod album_images;", True),
    ("sns-download-module", "src/toolkit/sns/mod.rs", "pub(crate) mod download;", True),
    ("sns-download-cli-flag", "src/cli/toolkit.rs", "download_media: bool,", True),
    ("sns-download-explicit-consent", "src/cli/export_sns.rs", "download_media.then(crate::toolkit::sns::DownloadOptions::default)", True),
    ("sns-media-export-cli", "src/cli/export_sns.rs", "crate::toolkit::sns::export_database_with_media(", True),
    ("sns-media-shared-staging", "src/toolkit/sns/export.rs", "write_export_with_media(&data, output, options.timezone, cache, download_options)", True),
    ("sns-staging-download-guard", "src/toolkit/sns/export.rs", "HostOutputGuard::new(staging.path())", True),
    ("sns-download-dispatch", "src/toolkit/sns/export.rs", "download::download(url, &destination, guard, options)", True),
    ("sns-fresh-only", "src/toolkit/sns/export.rs", "SNS destination already exists; use a fresh output directory", True),
    ("sns-contact-publish", "src/toolkit/sns/export.rs", 'fs::rename(staging.path(), &dir).context("publish SNS contact directory")?', True),
    ("sns-album-native-entry", "src/cli/sns_album.rs", "album::export(posts, &options, cache.as_ref())?", True),
    ("sns-album-six-reads", "src/cli/sns_album.rs", "for attempt in 1..=6", True),
    ("sns-album-read-timeout", "src/cli/sns_album.rs", "Duration::from_secs(30)", True),
    ("sns-album-read-budget", "src/cli/sns_album.rs", "256 * 1024 * 1024", True),
    ("sns-album-video-index", "src/cli/sns_album.rs", "cache::build_video_cache_index(", True),
    ("sns-feed-resolved", "src/daemon/query.rs", '"resolved_user": resolved_user', True),
    ("sns-feed-scan", "src/daemon/query.rs", '"scanned": scanned, "scan_truncated": scan_truncated', True),
    ("sns-album-image-existing", "src/toolkit/sns/album.rs", "album_images::reuse_existing_image(&name, existing)?", True),
    ("sns-album-image-url", "src/toolkit/sns/album.rs", '("url", "url_key", "url_token")', True),
    ("sns-album-image-thumb", "src/toolkit/sns/album.rs", '("thumb", "thumb_key", "thumb_token")', True),
    ("sns-album-video-existing", "src/toolkit/sns/album.rs", "album_videos::reuse_existing_video(&name, existing)?", True),
    ("sns-album-video-full", "src/toolkit/sns/album.rs", "copy_cached_video(&entry.path, &name, staging, false)", True),
    ("sns-album-video-remote", "src/toolkit/sns/album.rs", "album_videos::download_video(", True),
    ("sns-album-video-partial", "src/toolkit/sns/album.rs", "copy_cached_video(&entry.path, &name, staging, true)", True),
    ("sns-album-lazy-wasm", "src/toolkit/sns/album.rs", "get_or_init(|| VideoRuntime::bundled(Default::default()).ok())", True),
    ("sns-album-render", "src/toolkit/sns/album.rs", "album_render::render(&options.user, &posts)?", True),
    ("sns-album-long-title", "src/toolkit/sns/album_render.rs", "h1{overflow-wrap:anywhere}", True),
    ("sns-album-publish", "src/toolkit/sns/album.rs", "tree.publish_all(&entries)?", True),
    ("sns-album-bounded-legacy", "src/toolkit/sns/publish.rs", ".take(limit.saturating_add(1))", True),
    ("sns-album-existing-bytes", "src/toolkit/sns/album.rs", "album_videos::VideoSource::Remote | album_videos::VideoSource::Existing", True),
    ("sns-publish-adopt", "src/toolkit/sns/publish.rs", "unbound nonempty SNS output requires Adopt", True),
    ("sns-publish-unverified", "src/toolkit/sns/publish.rs", "legacy_unverified: nonempty", True),
    ("sns-publish-bounded-binding", "src/toolkit/sns/publish.rs", ".take(MAX_MANIFEST + 1).read_to_end(&mut bytes)?", True),
    ("sns-publish-atomic-file", "src/toolkit/sns/publish.rs", ".persist(&target)", True),
    ("sns-publish-no-rollback", "src/toolkit/sns/publish.rs", "earlier files were not rolled back", True),
    ("sns-publish-mtime", "src/toolkit/sns/publish.rs", ".set_times(fs::FileTimes::new().set_modified(*modified))?", True),
    ("sns-cache-video-only", "src/toolkit/sns/cache.rs", "pub fn build_video_cache_index(root: &Path, limits: CacheLimits)", True),
    ("sns-native-timeline", "src/cli/toolkit.rs", 'ToolkitCommands::ExportSns(args) => super::sns_timeline::cmd(args)', True),
    ("sns-timeline-account", "src/cli/sns_timeline.rs", 'source_kind: "account".into()', True),
    ("sns-timeline-flat", "src/cli/sns_timeline.rs", 'flat_cache: true', True),
    ("sns-static-nested", "src/cli/export_sns.rs", 'flat_cache: false', True),
    ("sns-timeline-shared-publish", "src/toolkit/sns/export.rs", 'tree.publish_all(&entries)?', True),
    ("sns-export-not-album-images", "src/toolkit/sns/export.rs", "album_images", False),
    ("run-status-dispatch", "src/cli/toolkit.rs", 'matches!(command.as_str(), "status" | "-s")', True),
    ("run-status-inspect", "src/cli/toolkit.rs", "native::run_status::inspect(", True),
    ("run-status-truthiness", "src/toolkit/run_status.rs", "fn truthy(value: &Value) -> bool", True),
    ("emoticons-offline-keys", "src/cli/toolkit.rs", "super::toolkit_run_prepare::load_saved(&runtime)?", True),
    ("emoticons-run-prepare", "src/cli/toolkit.rs", "super::toolkit_run_prepare::prepare()?", True),
    ("emoticons-process-check", "src/cli/toolkit_run_prepare.rs", "check(&runtime.config.wechat_process)?", True),
    ("emoticons-saved-keys", "src/cli/toolkit_run_prepare.rs", "let keys = match load_saved(&runtime)", True),
    ("emoticons-scan-validation", "src/cli/toolkit_run_prepare.rs", "validate_keys(&runtime, &keys)?", True),
    ("emoticons-atomic-save", "src/cli/toolkit_run_prepare.rs", ".persist(target)", True),
    ("emoticons-cache-isolation", "src/cli/export_emoticons.rs", 'runtime.cache_dir().join("emoticons")', True),
    ("emoticons-db-cache", "src/cli/export_emoticons.rs", "let cache = DbCache::with_dirs(", True),
    ("emoticons-catalog", "src/cli/export_emoticons.rs", "catalog::load(&cache).await", True),
    ("emoticons-preview", "src/cli/export_emoticons.rs", 'print!("{}", preview(&items))', True),
    ("emoticons-guard", "src/cli/export_emoticons.rs", "HostOutputGuard::new(&output)?", True),
    ("emoticons-download", "src/cli/export_emoticons.rs", "download(&item.md5, &item.info, &guard, &options)", True),
    ("emoticons-http-aes", "src/toolkit/emoticons/download.rs", "decrypt(&fetch(&client, &info.encrypt_url, opts)?, &info.aes_key)?", True),
    ("emoticons-hevc", "src/toolkit/emoticons/download.rs", "match convert_hevc_to_jpeg(&data, guard, opts)", True),
    ("emoticons-publish", "src/toolkit/emoticons/download.rs", ".persist_noclobber(&path)", True),
    ("mcp-root", "src/main.rs", "mod mcp;", True),
    ("mcp-cli", "src/cli/mod.rs", "Commands::Mcp(args) => mcp::cmd(args)", True),
    ("mcp-explicit-account", "src/cli/mcp.rs", 'std::env::var_os("WX_CLI_CONFIG")', True),
    ("mcp-bounded-ipc", "src/cli/mcp.rs", "transport::send_with_limits(", True),
    ("mcp-account-lock", "src/cli/mcp.rs", "let mut file = open_config_read_lock(&path)?;", True),
    ("delta-root", "src/toolkit/mod.rs", "mod chat_delta;", True),
    ("delta-cli", "src/cli/toolkit.rs", "ToolkitCommands::ExportDeltaNative(args) => super::export_delta::cmd(args)", True),
    ("delta-ipc", "src/daemon/server.rs", "query::q_export_delta_username(", True),
    ("delta-raw", "src/daemon/query/export_delta.rs", "raw_and_decoded(row.get_ref(4)?, row.get(5)?)", True),
    ("delta-new-root", "src/cli/export_delta.rs", "DeltaRunWriter::create(output_root, window)?", True),
    ("delta-manifest", "src/cli/export_delta.rs", "writer.finish()?", True),
    ("delta-append-dispatch", "src/cli/export_delta.rs", "let report = if args.append_run {", True),
    ("delta-append-writer", "src/cli/export_delta.rs", "DeltaRunWriter::create_run_in_existing_root(", True),
    ("delta-exclusive-run", "src/toolkit/chat_delta.rs", 'fs::create_dir(&run).context("delta run must be new and exclusive")?;', True),
    ("delta-no-clobber", "src/toolkit/chat_delta.rs", ".persist_noclobber(parent.join(filename))", True),
    ("plan-cli", "src/cli/toolkit.rs", "ToolkitCommands::ChatPlanNative(args) => super::chat_plan::cmd(args)", True),
    ("plan-scan", "src/cli/chat_plan.rs", "plan::collect_plan_with_scan(", True),
    ("plan-csv", "src/cli/chat_plan.rs", "plan::render_plan_csv(&rows)?", True),
    ("plan-ancestor-guard", "src/toolkit/chat_plan.rs", "let (_root_pins, root_status) = pin_scan_root(decrypted_dir)?;", True),
    ("asr-file-cli", "src/cli/toolkit.rs", "ToolkitCommands::TranscribeChatNative(args)", True),
    ("asr-manifest", "src/cli/asr.rs", "OfflineMedia::from_manifest(", True),
    ("asr-database-module", "src/cli/mod.rs", "mod asr_database;", True),
    ("asr-database-dispatch", "src/cli/toolkit.rs", "super::asr_database::cmd_transcribe_database_native(args)", True),
    ("asr-database-resolve", "src/cli/asr_database.rs", "let voice = database_media::resolve_voice(", True),
    ("asr-database-bytes", "src/cli/asr_database.rs", "transcribe_audio_bytes(&voice.silk, &backend)?", True),
    ("asr-unverified-account", "src/cli/asr_database.rs", '"account_authenticated": false', True),
    ("asr-message-identity", "src/toolkit/asr/database_media.rs", "WHERE local_id=?1 AND (?2 IS NULL OR create_time=?2) LIMIT 2", True),
    ("asr-media-identity", "src/toolkit/asr/database_media.rs", "FROM VoiceInfo WHERE chat_name_id=?1 AND svr_id=?2 LIMIT 2", True),
    ("asr-time-evidence", "src/toolkit/asr/database_media.rs", "if media_time != create_time {", True),
    ("asr-bytes-decoder", "src/toolkit/asr/mod.rs", "crate::toolkit::audio::decode_silk_to_pcm(bytes)?", True),
    ("asr-cache-registered", "src/toolkit/asr/mod.rs", "pub mod cache;", True),
    ("asr-cached-registered", "src/toolkit/asr/mod.rs", "pub mod cached;", True),
    ("asr-cache-dispatch", "src/cli/asr_database.rs", "cached::transcribe_cached(", True),
    ("asr-cache-authorize", "src/toolkit/asr/cached.rs", "backend.check_authorization()?;", True),
    ("asr-cache-success", "src/toolkit/asr/cached.rs", ".store_success_checked(&key, &record", True),
    ("host-checked-cache", "src/cli/mcp_voice.rs", "cached::transcribe_cached_with_receipt_checked(", True),
    ("host-receipt-fast-path", "src/cli/mcp.rs", "pending.try_cached(chat, context, current)", True),
    ("receipt-identity", "src/toolkit/asr/receipt.rs", "cached::identity(backend, receipt.proof.evidence.create_time)", True),
    ("image-metadata-request", "src/mcp/protocol.rs", 'mapped.insert("image_metadata".into(), json!(true))', True),
    ("image-metadata-query", "src/daemon/query.rs", "pub async fn q_attachments_with_image_metadata(", True),
    ("image-metadata-uniqueness", "src/daemon/query.rs", "SELECT COUNT(*) FROM (SELECT 1 FROM [{}] WHERE local_id=?1 AND create_time=?2 AND local_type=?3 LIMIT 2)", True),
    ("image-metadata-reader", "src/attachment/image_metadata.rs", "let reader = ResourceReader::open(path)?;", True),
    ("image-metadata-scan", "src/attachment/image_metadata.rs", "scan_candidates(&mut scan, &chat, &hashes)?", True),
    ("image-metadata-projection", "src/mcp/protocol.rs", '"size_kind": "encrypted_dat_metadata"', True),
    ("cache-preflight", "src/toolkit/asr/cached.rs", "check(&transcription)?;", True),
    ("cache-final-callback", "src/toolkit/asr/cache.rs", "before_commit()?;", True),
    ("mcp-voice-registered", "src/daemon/query.rs", "mod mcp_voice;", True),
    ("mcp-voice-query", "src/daemon/server.rs", "query::mcp_voice::q_voice_messages(", True),
    ("mcp-tags-query", "src/daemon/server.rs", "query::mcp_contacts::q_contact_tags(", True),
    ("strict-message-registered", "src/daemon/query.rs", "mod strict_message;", True),
    ("refer-strict-message", "src/daemon/query/mcp_refer.rs", "strict_message::locate(", True),
    ("attachments-strict-message", "src/daemon/query/mcp_attachments.rs", "strict_message::locate(", True),
    ("attachments-server", "src/daemon/server.rs", "query::mcp_attachments::q_attachment_reference(", True),
    ("attachments-reference", "src/daemon/query/mcp_attachments.rs", "attachment_refs::find_reference(&base, &metadata)", True),
    ("attachment-scalar-elements-rejected", "src/toolkit/attachment_refs.rs", 'node.children().any(|n| n.is_element())', True),
    ("attachment-file-scalar-md5", "src/toolkit/attachment_refs.rs", 'hash(&scalar_text(child(app, "md5")?)?)?', True),
    ("attachment-record-scalar-md5", "src/toolkit/attachment_refs.rs", 'hash(&scalar_text(child(item, "fullmd5")?)?)?', True),
    ("strict-complete-inventory", "src/daemon/query/strict_message.rs", "ensure_complete_message_inventory(db, names)?;", True),
    ("native-image-public", "src/attachment/mod.rs", "mod native_image;", True),
    ("mcp-image-public", "src/daemon/query.rs", "mod mcp_image;", True),
    ("image-host-policy", "src/cli/mcp.rs", "policy.prepare_request(&mut request)?;", True),
    ("image-server", "src/daemon/server.rs", "query::mcp_image::q_decode_image_with_key_file(", True),
    ("image-host-guard", "src/daemon/query/mcp_image.rs", "native_image::HostOutputGuard::new(output_root)", True),
    ("image-strict-message", "src/daemon/query/mcp_image.rs", "strict_message::locate(", True),
    ("image-native-export", "src/daemon/query/mcp_image.rs", "native_image::export_image_with_guard(", True),
    ("image-original-guard-recheck", "src/attachment/native_image.rs", "export_image_impl(request, Some(guard))", True),
    ("image-no-clobber", "src/attachment/native_image.rs", "temp.persist_noclobber(&path)", True),
    ("image-business-error", "src/daemon/server.rs", '"message": "Image export failed"', True),
    ("image-list-metadata", "src/mcp/protocol.rs", "let mut image = image_metadata(row)?;", True),
    ("mcp-audio-public", "src/daemon/query.rs", "mod mcp_audio;", True),
    ("voice-host-public", "src/cli/mod.rs", "mod mcp_voice;", True),
    ("voice-host-prepare", "src/cli/mcp.rs", "policy.voice.prepare(", True),
    ("voice-daemon-prepare", "src/daemon/server.rs", "query::mcp_audio::q_prepare_voice(", True),
    ("voice-internal-budget", "src/ipc.rs", "24 * 1024 * 1024", True),
    ("voice-host-evidence", "src/cli/mcp_voice.rs", "prepared_audio::decode(", True),
    ("voice-host-publish", "src/cli/mcp_voice.rs", "publish_wav_noclobber(&wav, guard", True),
    ("voice-host-text-budget", "src/cli/mcp_voice.rs", ".check_text_result(&text)", True),
    ("voice-real-request-id", "src/mcp/protocol.rs", "self.response_id.clone()", True),
    ("voice-private-temp", "src/cli/mcp_voice.rs", '.prefix("wx-cli-mcp-voice-")', True),
    ("voice-local-deadline", "src/cli/mcp_voice.rs", "config.timeout = config.timeout.min(remaining)", True),
    ("voice-cloud-deadline", "src/toolkit/asr/openai.rs", ".timeout(self.timeout)", True),
    ("voice-cache-timeout-independent", "src/toolkit/asr/cached.rs", "config.timeout.as_nanos()", False),
    ("history-cli-request", "src/cli/history.rs", "let req = Request::History {", True),
    ("history-ipc-types", "src/ipc.rs", "msg_types: Option<Vec<i64>>", True),
    ("history-ipc-order", "src/ipc.rs", "oldest_first: bool", True),
    ("history-server-query", "src/daemon/server.rs", "match query::q_history(", True),
    ("history-mcp-types", "src/mcp/protocol.rs", 'mapped.insert("msg_types".into(), json!(resolved));', True),
    ("history-selection", "src/daemon/query.rs", "history_selection::Selection::new(", True),
    ("history-shard-mapper", "src/daemon/query.rs", "selection.query_shard(&conn, &tname, read_history_row)?", True),
    ("history-shared-render", "src/daemon/query.rs", "return Ok(render_history_rows(", True),
    ("history-global-page", "src/daemon/query.rs", "selection.page(", True),
    ("history-page-display", "src/daemon/query/history_selection.rs", "page.sort_by_key(|entry| entry.timestamp);", True),
    ("wasm-cli", "src/cli/toolkit.rs", "ToolkitCommands::DecodeSnsVideo", True),
    ("wasm-runtime", "src/toolkit/sns/mod.rs", "mod video_runtime;", True),
]
PALETTE = {"live": ("#f0fdf4", "#15803d", "已接入"),
           "pending": ("#fff7ed", "#c2410c", "开发中 / 待接线"),
           "legacy": ("#fef2f2", "#b91c1c", "Python / Node"),
           "target": ("#eff6ff", "#2563eb", "能力边界 / 待验收")}


def hashes():
    return {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCES}


def verify_wiring():
    anchors = []
    for claim, path, token, required in WIRING:
        text = (ROOT / path).read_text(encoding="utf-8")
        if (token in text) != required:
            sys.exit(f"STALE wiring claims: review {claim}: {path}: {token}")
        anchors.append(dict(claim=claim, path=path, token=token, expected_present=required,
                            scope="audited_scope",
                            line=next((i for i, line in enumerate(text.splitlines(), 1) if token in line), None)))
    protocol = (ROOT / "src/mcp/protocol.rs").read_text(encoding="utf-8")
    body = protocol.split("pub fn tools() -> Vec<Tool> {", 1)[1].split("for tool in &mut out", 1)[0]
    entries = list(re.finditer(r'^\s*\(\s*\n\s*"([a-z_]+)",', body, re.MULTILINE))
    entries += list(re.finditer(r'Tool\s*\{\s*name:\s*"([a-z_]+)"', body))
    names = [entry.group(1) for entry in sorted(entries, key=lambda match: match.start())]
    if names != MCP_TOOLS:
        sys.exit("STALE MCP tools: re-audit coverage before rendering: " + repr(names))
    return anchors


def node(id_, title, lines, state="live"):
    return dict(id=id_, title=title, lines=lines, state=state)


def diagrams():
    n = node
    return [
        dict(name="runtime-current", title="wx-cli / wx-toolbox 当前运行架构", height=6540,
             snapshot="2026-09-07 / 双 bin check5 通过；主单元794/0/7；全量未收口",
             subtitle="Windows x64 MSVC | 已接入指源码调用可达，不等于已安装或已发布 | 功能总图",
             rows=[
                 ("01 进程与账号隔离", [
                     n("main", "共享双 binary 入口", ["wx：main；toolbox：include main", "toolbox 无参数默认本地 GUI", "WX_DAEMON_MODE 优先分流"]),
                     n("context", "固定账号上下文", ["runtime · RuntimeContext", "配置路径 + 数据库 + 密钥路径", "运行根目录 + 协议代次"]),
                     n("pipe", "账号独立命名管道", ["cli/transport · send", "后台启动 / 停止与存活锁", "每账号 cache / PID / log"]),
                     n("query", "后台请求分派", ["daemon/server → query", "账号解密缓存 + WAL 刷新", "基础查询不启动 Python"]),
                 ], ["上下文", "请求", "IPC"]),
                 ("02 wx mcp：stdio → 固定显式账号 → 有界 IPC（17 tools，不是旧语义全覆盖）", [
                     n("mcpcli", "原生 MCP 入口", ["main：mod mcp；CLI：wx mcp", "mcp/protocol：stdio JSON-RPC", "初始化 / 列表不读账号"]),
                     n("mcpscope", "首次查询固定账号", ["cli/mcp · PinnedAccount", "显式 WX_CLI_CONFIG + db_dir", "配置读锁 / 账号变化即失效"]),
                     n("mcpipc", "有界账号 IPC", ["transport · send_with_limits", "剩余期限 + 响应字节上限", "复用既有后台启动边界"]),
                     n("mcptools", "17 项工具路由", ["14 只读 + 图片 / WAV / ASR", "语音：daemon准备，host执行", "同步服务未支持在途取消"]),
                 ], ["查询", "请求", "IPC"]),
                 ("03 get_chat_images：专用元数据查询 → 页内身份的全分片唯一性 → 资源与 DAT 元数据", [
                     n("imagemetarequest", "图片列表内部请求", ["get_chat_images", "image_metadata=true（内部）", "普通 CLI 附件查询保持轻量"]),
                     n("imagemetaquery", "分页后全分片唯一性", ["q_attachments_with_", "image_metadata", "COUNT / LIMIT 2；歧义不选行"]),
                     n("imagemetareader", "共享资源读取与候选扫描", ["ResourceReader：精确行 / 事务", "scan_candidates：每页一次扫描", "Scan / Pin 固定目录与文件"]),
                     n("imagemetaresult", "白名单元数据与状态", ["资源 MD5 / 加密 DAT 长度", "metadata.len；缺失或歧义为null", "不读 DAT 正文、不解码或上传"]),
                 ], ["查询", "唯一身份", "投影"]),
                 ("04 引用与附件：同账号严格消息定位后解析；标签与语音列表为独立 query 分支", [
                     n("strict", "唯一消息与完整分片", ["query/strict_message · locate", "先检查唯一性，再检查类型", "存储 1 MiB；解压按入口限额"]),
                     n("attmeta", "文件与转发项元数据", ["query/mcp_attachments", "文件 20K / 记录 500K 字节", "XML 6 / 19；原始零基索引"]),
                     n("attrefs", "账号内原始附件引用", ["toolkit/attachment_refs", "db_dir.parent → msg/file 或 Rec", "受限扫描 / 只读句柄 / MD5"]),
                     n("attresult", "元数据、状态与引用", ["found / missing / text", "metadata_only；无 hash 警告", "不解码、不上传、不写附件"]),
                 ], ["唯一行", "元数据", "引用"]),
                 ("05 decode_image：宿主授权本地写出；真实进程与合成加密账号通过，用户私有账号未验", [
                     n("imagehost", "宿主路径与密钥策略", ["media-output-root 必须预存", "image-key-file 仅宿主提供", "工具参数不接收路径或密钥"]),
                     n("imagequery", "严格消息与账号资源", ["server → query/mcp_image", "strict_message：唯一图片", "资源清单 / 受限 DAT 候选"]),
                     n("imagepublish", "解码及不覆盖发布", ["native_image → decoder", "暂存 / sync / persist_noclobber", "不扫描密钥、不下载或上传"]),
                     n("imageresponse", "发布与响应分离", ["响应超限或超时仍可失败", "已发布文件不回滚", "不承诺自动重试或幂等成功"], "target"),
                 ], ["IPC", "解码", "结果"]),
                 ("06 历史查询：CLI / MCP → History IPC → q_history → 全分片选页（不是新工具）", [
                     n("historyentry", "CLI 与 MCP 参数入口", ["cli/history / mcp/protocol", "msg_types 并集 / oldest_first", "与单 msg_type 互斥校验"]),
                     n("historyipc", "实际 IPC 与后台分派", ["Request::History → server", "q_history → Selection::new", "多类型或最早页启用新分支"]),
                     n("historyselect", "分片选行与共享映射", ["history_selection · query_shard", "read_history_row / 共享渲染", "每片候选 limit + offset"]),
                     n("historypage", "全分片合并与分页", ["Selection::page：统一 offset", "最早 / 最新页；页内升序", "不按 local_id 去重或造游标"]),
                 ], ["参数", "候选", "合并"]),
                 ("07 export-delta-native：默认新 root / --append-run 既有 root；只创建新批次", [
                     n("deltacli", "已注册 Delta CLI", ["cli/export_delta · cmd", "显式 --append-run 复用 root", "输入与输出边界先校验"]),
                     n("deltaquery", "专用 IPC 与原始正文", ["Request::ExportDelta", "query/export_delta", "逻辑分片 + raw_content"]),
                     n("deltauid", "稳定 UID 与增量模型", ["chat_delta · prepare_delta", "用户名 / 来源 / ID / 时间", "原始内容哈希参与 msg_uid"]),
                     n("manifest", "独占 run 与完成清单", ["deltas/run_id：独占新建", "逐聊天不覆盖 + manifest", "不改原完整输出或已有 run"]),
                 ], ["IPC", "原始模型", "发布"]),
                 ("08 chat-plan-native：显式离线输入 → estimate / scan → 原契约 12 列 CSV", [
                     n("plancli", "已注册离线计划入口", ["cli/chat_plan · Args / cmd", "解密根 / 库路径 / username", "可选 JSON 元数据与精确过滤"]),
                     n("planstats", "只读统计与实际扫描", ["消息正文 / 资源 / 语音估算", "scan：源或媒体目录；1–6线程", "解密根完整祖先链拒绝 junction"]),
                     n("plancsv", "完整 CSV 值与空值", ["render_plan_csv：原 12 列", "UTF-8 BOM / CRLF / 整数", "缺表、缺库与扫描状态可见"]),
                     n("planpublish", "原子且不覆盖发布", ["NamedTempFile → no-clobber", "输出不能进入源或解密缓存", "不是聊天导出或计划执行器"]),
                 ], ["输入", "统计值", "写入"]),
                 ("09 其他已接入能力（并列展示，不表示相互调用）", [
                     n("batch", "全量与增量 JSON 预览", ["export-chats-native / chat_index", "ExportUsername / 联系人字段", "chat_merge 与 Delta 入口并存"]),
                     n("db", "初始化、数据库与图片", ["scanner/windows / DPAPI", "crypto：页认证 / 解密", "images → attachment/decoder"]),
                     n("audio", "语音转换与显式 ASR", ["CLI 与 MCP host 均已接线", "内部24MiB / 独立MCP预算", "细节见 mcp-voice-flow 图"]),
                     n("sns", "SNS / WASM / 企业快照", ["SNS 导出与相册：见13–16行", "decode-sns-video：离线 WASM", "enterprise：只读查询 / 导出"]),
                 ], []),
                 ("10 run status / -s：只读统计；原 toolkit status 环境报告不变", [
                     n("statuscli", "原生状态 CLI", ["toolkit run status / -s", "-- 后 JSON / exported-dir", "cli/toolkit → run_status"]),
                     n("statusconfig", "选中配置与相对路径", ["run_status::inspect", "相对选中 config 所在目录", "不建缺失目录、不写配置"]),
                     n("statuscounts", "元数据与转录统计", ["密钥仅元数据；DB / JSON字节", "messages / chats：truthiness", "不读取密钥内容、不改转录"]),
                     n("statusresult", "文本 / JSON 与警告", ["消息库 MB = 全部实际大小和", "坏转录单列 warning", "不将部分统计假报完整"]),
                 ], ["配置", "只读", "报告"]),
                 ("11 表情入口与准备：run 检查进程；offline 使用已保存 keys；两路汇入同一导出器", [
                     n("emojicli", "表情原生 CLI", ["toolkit run emoticons", "或 toolkit export-emoticons", "help 在配置与进程检查之前"]),
                     n("emojiprepare", "prepare / offline", ["run：进程名检查 / saved keys", "需新扫描：验证后原子保存", "offline：load_saved / 路径校验"]),
                     n("emojicache", "账号独立 DbCache", ["export_emoticons::export", "emoticons.lock / with_dirs", "隔离 cache/emoticons"]),
                     n("emojicatalog", "Rust 表情目录", ["emoticons/catalog::load", "DbCache 解密加密 catalog", "NonStore / Store / 描述映射"]),
                 ], ["分流", "keys", "读取"]),
                 ("12 承接 catalog：过滤 / 预览，或守卫下载发布；原生流程不依赖 Python", [
                     n("emojifilter", "过滤与预览分支", ["描述 / product_id 忽略大小写", "dry-run：预览后返回", "不下载、不创建导出目录"]),
                     n("emojiguard", "下载前输出守卫", ["配置旁 exported_emoticons", "validate_output / HostGuard", "保护源 / 配置 / keys / cache"]),
                     n("emojidownload", "HTTP / AES / HEVC", ["缓存复用；受限 HTTP 下载", "CDN失败回退加密URL / AES", "HEVC：可选ffmpeg，失败bin"]),
                     n("emojiimages", "图片与回退输出", ["暂存 / sync / 守卫复核", "图片不覆盖；bin 可替换", "逐项失败计数，批次 exit 0"]),
                 ], ["非预览", "下载", "发布"]),
                 ("13 原生时间线：export-sns 固定账号更新；export-sns-native 显式静态库，默认 fresh", [
                     n("snsmedia_cli", "cli/sns_timeline", ["export-sns；不启动 Python", "账号绑定 / 默认 Update", "adopt-existing 显式认领"]),
                     n("snsmedia_stage", "共享 export.rs", ["静态入口 --update 可更新", "全联系人绑定预检后暂存", "缓存恢复优先；下载须授权"]),
                     n("snsmedia_download", "缓存布局与媒体", ["生产缓存平铺 SNS/", "静态缓存 images/ 与 videos/", "download / 守卫 / 状态计数"]),
                     n("snsmedia_publish", "时间线发布", ["更新：publish_all 逐文件替换", "fresh：完整联系人目录 rename", "不删旧文件；失败不回滚"]),
                 ], ["调用", "恢复", "发布"]),
                 ("14 原生 SNS 相册：固定账号只读 IPC → 精确作者 → 编排；最多六次尝试，非整批30秒", [
                     n("albumcli", "原生相册 CLI", ["cli/sns_album → album::export", "固定 RuntimeContext", "不启动 Python / Node / CLI"]),
                     n("albumfeed", "固定账号有界 Feed", ["send_with_limits；最多6次", "每次 30s / 256 MiB", "失败退避；只重试读取"]),
                     n("albumidentity", "精确作者与扫描证据", ["q_sns_feed：resolved_user", "scanned / scan_truncated", "缺作者拒绝；截断警告"]),
                     n("albumjobs", "媒体编排与懒加载", ["album / images / videos / render", "图片1–32；视频1–16线程", "每线程按需加载 WASM"]),
                 ], ["查询", "Feed", "帖子"]),
                 ("15 相册媒体顺序：图片不扫描 cache；视频仅使用本账号 video-only 索引（两条独立链）", [
                     n("albumimage", "图片复用与回退", ["existing → url → thumb", "album_images；image_cache=0", "明文图片不加载 WASM"]),
                     n("albumimageout", "图片暂存与引用", ["受限下载 / 格式识别", "images/ 本地引用及错误", "no-remote：仅复用已有"]),
                     n("albumvideo", "视频复用与回退", ["existing → 完整cache", "→ remote → partial cache", "build_video_cache_index"]),
                     n("albumvideoout", "视频暂存与状态", ["complete / partial 分开统计", "existing / remote：video_bytes", "明文直通；加密懒加载 WASM"]),
                 ], ["暂存", None, "暂存"]),
                 ("16 相册发布：先绑定预检，再兄弟目录暂存；逐文件原子替换，不提供整树事务", [
                     n("albumbinding", "输出树来源绑定", ["publish::prepare；输入保护", "同账号 / 作者可更新", "无绑定旧目录须 explicit adopt"]),
                     n("albumstage", "输出树外暂存", [".wx-album-* / album_render", "媒体 → JSON → HTML → summary", "timeline 保持数组形状"]),
                     n("albumpublish", "逐文件原子发布", ["同目录暂存 / sync / persist", "复核目录、源、当前目标", "失败不回滚；保留源 mtime"]),
                     n("albumlimits", "更新与认领边界", ["不删除未知旧文件", "adopt：历史来源未核验", "绑定不是完成标记或认证"], "target"),
                 ], ["预检后", "逐文件", "边界"]),
                 ("17 setup 与 init：共享配置快照；向导只配置，init 才进入扫描（并列分支）", [
                     n("setupcli", "setup_native / launcher", ["默认预览；apply 明确确认", "首次无配置且 TTY 才交互", "取消不继续启动 GUI"]),
                     n("setupcore", "toolkit/setup", ["ConfigDocument / Snapshot", "保留未知字段 / 复核原快照", "不扫描、不加载模型"]),
                     n("initcli", "cli/init", ["显式或既有账号优先", "坏配置在扫描前拒绝", "scanner 验证后保存 keys"]),
                     n("initcommit", "分文件原子提交", ["keys 后 config；非整体事务", "配置失败明确提示已存 key", "只停止身份匹配的旧后台"]),
                 ], ["配置", None, "提交"]),
                 ("18 原生批量转录：transcribe-chat / export_all 显式请求；三种后端不自动互相回退", [
                     n("asrbatch", "asr_batch → asr/batch", ["固定账号；精确媒体库关联", "成功缓存 / receipt / JSON回写", "既有 transcription 保留"]),
                     n("python", "local：Python 兼容推理", ["local_python：Whisper / Torch", "Rust 管理进程、缓存与回写", "命名模型仍可按需下载权重"], "legacy"),
                     n("cppbatch", "whisper_cpp：本地进程", ["Rust Local / Job Object", "显式模型或兼容查找已有模型", "不下载模型、不需要 Python"]),
                     n("cloudbatch", "openai：显式上传", ["allow-upload 先于读凭据", "环境变量 / 显式文件 / 旧key", "失败不切后端、不回显凭据"]),
                 ], []),
                 ("19 本地 Web / GUI：Rust Axum + 内嵌 HTML/CSS/原生 JavaScript；不是 Node 或 Python 服务", [
                     n("webentry", "cli/web_native", ["web；gui 设置 open=true", "固定 RuntimeContext / Tokio", "默认系统浏览器界面"]),
                     n("webserver", "web/server：Axum", ["仅127.0.0.1 / 默认随机端口", "Host / Origin / token / CSRF", "assets 经 include_bytes 内嵌"]),
                     n("webquery", "query / preview", ["普通微信只读账号 IPC", "企业微信固定离线快照", "受限历史 / 标签 / 图片预览"]),
                     n("webui", "浏览器 UI 与能力限制", ["联系人 / 记录 / 图片 / 任务", "不等于所有 CLI 与 MCP 工具", "队列底栏修复后 QA 复跑中"], "target"),
                 ], ["启动", "查询", "结果"]),
                 ("20 Web 写任务：显式授权与白名单参数 → 有界串行 worker → 同程序子进程 → SSE 状态", [
                     n("webtasks", "web/tasks", ["解密 / 导出 / SNS / MP3", "普通与企业取钥逐任务授权", "固定输入，限制选项与范围"]),
                     n("webworker", "worker / process", ["有界队列 / 串行执行", "复用本程序 CLI 参数计划", "Job Object 管理进程树"]),
                     n("weblog", "日志与任务状态", ["stdout/stderr 有界读取脱敏", "取钥子进程输出完全抑制", "成功 / 失败 / 取消 / SSE"]),
                     n("webcancel", "取消与关闭", ["终止并回收子进程树", "不回滚此前发布的文件", "任务成功不保证每项媒体成功"], "target"),
                 ], ["入队", "执行", "控制"]),
                 ("21 cleanup：默认只读计划；执行必须固定账号、审阅计划和精确文件选择", [
                     n("cleanupcli", "cleanup_native", ["配置与 runtime 根只选一次", "默认 status / plan 不创建锁", "空间统计错误显示 partial"]),
                     n("cleanupplan", "cleanup：绑定文件计划", ["原生 inventory / 显式旧接管", "write-plan：新文件不覆盖", "源 / 配置 / key 路径保护"]),
                     n("cleanupselect", "显式执行与复核", ["plan + select + confirm-account", "精确 ID；拒绝 all / 通配符", "重核来源、目录和文件句柄"]),
                     n("cleanupresult", "逐文件删除与结果", ["旧接管需 authorize-legacy", "key 删除另行授权且两次确认", "非递归清空；部分失败非零"]),
                 ], ["规划", "审阅后", "删除"]),
                 ("22 企业微信批量与监控（两条独立链）；不扩张为全版本支持或 daemon 生命周期管理", [
                     n("workbatch", "enterprise_batch", ["发现 / 授权扫描 / 逐库 key", "账号绑定批量主文件解密", "多会话 JSON / CSV / HTML"]),
                     n("worklimits", "enterprise / 边界", ["wxSQLite3；主文件不合并WAL", "旧结构偏移非全64位兼容", "输入与输出身份单独校验"], "target"),
                     n("monitor", "monitor_native", ["已有daemon轮询 / 游标文件", "取消 / 终端 / 延迟分阶段", "不启动或停止账号 daemon"]),
                     n("monitorlimits", "monitor / latency", ["元数据 / Ping / Sessions", "可选History；不是内部SQL计时", "同秒游标不承诺绝对无遗漏"], "target"),
                 ], ["复用", None, "IPC"]),
                 ("23 当前回归与实际缺口：专项通过不代表全量、真实模型或发布验收完成（并列状态）", [
                     n("checks", "主任务提供的证据", ["两个bin check5通过 / 22警告", "主单元801：794通过/7忽略", "CLI63通过；ASR安全8通过"], "target"),
                     n("gaps", "扫描兼容修复进行中", ["候选64..192 / 跨库HMAC", "多PID / 同salt多DB", "只读审计实际缺口，未收口"], "pending"),
                     n("imagegaps", "图片兼容修复进行中", ["skipped_no_key / 格式对齐", "离线 UIN-MD5 helper 待接线", "offline 与内存模式互斥"], "pending"),
                     n("pending", "尚未完成的验收", ["新增9项setup测试尚未运行", "最终全量结果仍待确认", "UI修复复跑；无全目标完成"], "target"),
                 ], []),
             ], continuations=[("emojicatalog", "emojifilter")],
             notes=["已有 keys 不做全库 HMAC；新增扫描才验证保存。表情真实 run 测试复用 saved keys，未扫描真实进程内存。",
                    "只更新当前总图；专项图与 source-evidence 保留历史快照。实际兼容缺口修复中，源码证据暂不封存。"]),
        dict(name="sns-cache-publish", title="SNS 原生时间线、相册与发布边界", height=2220,
             snapshot="2026-09-07 / 时间线：20 targets，1142/0/11；check 待确认",
             subtitle="export-sns 账号更新 | export-sns-native 静态 fresh / update | sns-album 独立 Feed 与媒体策略",
             rows=[
                 ("01 原生入口与身份：固定 RuntimeContext，最多六次只读尝试，不启动 Python / Node", [
                     n("args", "cli/sns_album", ["cmd_sns_album → album::export", "日期 / 输出与输入隔离", "no-remote / no-videos"]),
                     n("read", "账号固定 IPC", ["send_with_limits；最多6次", "每次30s / 256MiB；失败退避", "不是整批30秒时限"]),
                     n("feed", "q_sns_feed 证据", ["resolved_user 精确作者", "scanned / scan_truncated", "缺作者拒绝；截断警告"]),
                     n("jobs", "album 媒体编排", ["images / videos / render", "图片1–32；视频1–16线程", "清除输入伪造的本地引用"]),
                 ], ["读取", "响应", "帖子"]),
                 ("02 图片严格优先序：existing → url → thumb；本相册不进行图片 cache 扫描", [
                     n("imgexisting", "existing", ["reuse_existing_image", "守卫下复用已有图片", "命中不下载；image_cache=0"]),
                     n("imgurl", "url", ["download_sns_image", "url_key / url_token", "受限响应读取与格式识别"]),
                     n("imgthumb", "thumb", ["仅原图未成功时回退", "thumb_key / thumb_token", "仍失败则 image_missing"]),
                     n("imgwasm", "暂存与按需解码", ["images/ 引用 → 帖子", "明文不需要 WASM", "加密时线程 OnceLock 初始化"]),
                 ], ["未命中", "失败", None]),
                 ("03 视频严格优先序：existing → 完整cache → remote → partial；不是任意缓存优先", [
                     n("videxisting", "existing", ["reuse_existing_video", "video_bytes 保存实际大小", "已有有效结果不下载"]),
                     n("vidcache", "完整 cache", ["build_video_cache_index", "本账号 Video；不扫描 Img", "copy_cached_video(..., false)"]),
                     n("vidremote", "remote", ["download_video；明文直通", "加密前缀按需 WASM 解码", "no-remote 禁用此步骤"]),
                     n("vidpartial", "partial cache", ["完整来源未成功才回退", "copy_cached_video(..., true)", "不完整单列；缺失不假报成功"]),
                 ], ["未命中", "未命中", "失败"]),
                 ("04 发布主链：来源预检 → 输出树外暂存 → 逐文件原子替换 → 可见结果（非整树事务）", [
                     n("binding", "publish::prepare", ["账号 / 作者 / tree_kind 绑定", "协作锁与全部候选预检", "绑定清单不是完成标记"]),
                     n("stage", "兄弟目录暂存", [".wx-album-* / album_render", "媒体及 timeline 数组 JSON", "timeline.html / summary"]),
                     n("publish", "publish_all", ["媒体 → JSON → HTML → 汇总", "同目录暂存 / sync / persist", "复核身份；复制保持 mtime"]),
                     n("result", "部分提交可见", ["失败不回滚已提交文件", "没有整树事务或跨文件 CAS", "更新不删除未知旧文件"], "target"),
                 ], ["预检后", "逐文件", "结果"]),
                 ("05 认领及资源边界（说明，不是额外调用链）", [
                     n("adopt", "explicit adopt", ["无绑定非空目录须显式认领", "已知账号 / 联系人冲突拒绝", "legacy_unverified=true"], "target"),
                     n("provenance", "历史来源未核验", ["认领旧媒体不是账号认证", "复用文件不证明历史来源", "未知旧文件和子树保持不动"], "target"),
                     n("bounded", "读取预算", ["旧timeline 256MiB / 汇总1MiB", "绑定清单64KiB；take(limit+1)", "不是仅检查初始 metadata"]),
                     n("tests", "本轮测试通过", ["20 targets：1142次通过", "0失败 / 11忽略；check待定", "host8 / core6 / CLI7 已包含"], "target"),
                 ], []),
                 ("06 独立入口与未迁移边界：以下并列，不是相册调用链", [
                     n("offline", "export-sns-native", ["显式库；默认 fresh rename", "--update；可 adopt-existing", "snapshot 绑定 / 缓存嵌套"]),
                     n("oldtimeline", "toolkit export-sns", ["cli/sns_timeline；无 Python", "固定账号 / 默认 Update", "account 绑定 / 缓存平铺"]),
                     n("oldflows", "其他旧工作流", ["export_all / transcribe_chat", "其他 run / GUI 仍有旧路径", "不宣称全量 Rust 迁移"], "legacy"),
                     n("wasm", "独立 ASR / 离线视频", ["ASR cache 不调用 SNS cache", "decode-sns-video 仍独立可用", "相册无需 Node 运行时"]),
                 ], []),
                 ("07 时间线更新链：共享 export.rs / publish；不是相册 Feed，也不是旧时间线 JSON 合并", [
                     n("timelinebind", "全部联系人预检", ["tree_kind=timeline / 精确作者", "账号或静态路径身份绑定", "adopt 不认证历史媒体"]),
                     n("timelinestage", "恢复与下载暂存", ["cache 优先 / 缺失项下载", "生产 flag 或 env=1 授权", "no-remote 优先禁止网络"]),
                     n("timelinepublish", "publish_all", ["媒体 / 单帖 / timeline / HTML", "_media_recovery.json 报告", "逐文件原子替换；非整树事务"]),
                     n("timelinelimits", "更新不等于历史合并", ["汇总由本轮数据库重建", "未计划旧文件不删除", "空筛选不改输出；失败可部分提交"], "target"),
                 ], ["全批通过", "暂存后", "边界"]),
             ],
             notes=["绿色为源码已接线；媒体回退箭头仅在前序失败时推进。WASM 懒加载，不把 ftyp 检查当作完整播放验证。",
                    "全量日志：wx-cli-sns-timeline-all-retest.log；1142含重复执行；check待确认，源码证据待封存。"]),
        dict(name="asr-wasm-wiring", title="ASR / WASM 与新增入口接线边界", height=2220,
             subtitle="文件与数据库 ASR、离线 WASM、MCP、Delta 追加已注册 | 绿色不等于旧功能全部迁移",
             rows=[
                 ("01 旧 ASR 仍兼容；SNS 相册已原生（两条独立入口）", [
                     n("oldcli", "旧转录命令", ["wx toolkit transcribe-chat", "cli/toolkit · TranscribeChat", "仍进入 run_script"], "legacy"),
                     n("oldasr", "Python 语音流程", ["transcribe_chat.py", "ASR 配置 / 模型 / 回写", "未宣称已完整替代"], "legacy"),
                     n("oldalbum", "原生 SNS 相册", ["cli/sns_album → sns/album", "图片 / 视频 / render / publish", "固定账号与来源绑定"]),
                     n("oldwasm", "按需 Rust WASM 宿主", ["VideoRuntime::bundled", "每线程 OnceLock；明文不加载", "不启动 Node；本阶段回归通过"]),
                 ], ["启动", None, "解码调用"]),
                 ("02 ASR 已注册：transcribe-audio-native / transcribe-chat-native", [
                     n("asrcli", "显式原生 CLI 参数", ["cli/asr · cmd_transcribe_*", "toolkit 与 CLI 均已注册", "不自动发现账号或下载模型"]),
                     n("prepare", "音频与媒体身份", ["asr · prepare_wav", "transcribe_audio / transcribe_chat", "OfflineMedia：分片 + local_id"]),
                     n("backend", "显式 ASR 后端", ["local::transcribe：模型子进程", "openai::transcribe_wav：HTTP", "远程上传需明确授权"]),
                     n("writeback", "结构化结果与回写", ["writeback::transcribe_file", "批次结束后原子发布", "当前 CLI 用显式媒体清单"]),
                 ], ["输入", "识别", "结果"]),
                 ("03 toolkit transcribe-database-native：严格消息 / VoiceInfo 映射 → 现有 ASR", [
                     n("asrdb", "显式静态快照入口", ["cli/asr_database → resolve_voice", "根 / username / source / local_id", "先检查后端与上传授权"]),
                     n("dbjoin", "严格语音身份与关联", ["完整消息分片 + 唯一语音行", "Name2Id / svr_id / 时间一致", "歧义拒绝；不按媒体 ID 猜分片"]),
                     n("dbbytes", "SILK 字节复用 ASR", ["transcribe_audio_bytes", "内存解码；不写临时 SILK", "本地仍用受控临时 WAV"]),
                     n("dbevidence", "转录 JSON 与来源证据", ["account_authenticated=false", "调用方提供快照；非账号认证", "不回写数据库；不输出音频"]),
                 ], ["身份", "字节", "结果"]),
                 ("04 数据库 ASR 可选缓存：--cache-file 与 --cache-account 成对显式启用", [
                     n("cachecli", "CLI 缓存预检与分派", ["asr_database → cached", "缓存不别名覆盖库或受保护输入", "不传参数则沿用字节转录"]),
                     n("cachekey", "授权先检与后端身份", ["消息来源 / 时间 / SILK 摘要", "模型与识别配置参与键", "执行 timeout 不参与 identity"]),
                     n("cachelookup", "账号隔离缓存读取", ["cache · lookup；成功空文本可命中", "命中仍须上传授权检查", "未命中 → transcribe_audio_bytes"]),
                     n("cachestore", "成功缓存与独立状态", ["store_success / 原子发布", "读写失败不推翻成功识别", "账号标签不是账号认证"]),
                 ], ["显式启用", "缓存键", "未命中"]),
                 ("05 WASM 独立入口：decode-sns-video；相册另经 album_images / album_videos 懒加载", [
                     n("videocli", "离线视频 CLI", ["cli/sns_video · cmd_decode", "显式 input / output / key-file", "只读保持源句柄"]),
                     n("runtime", "Rust 受限 WASM 宿主", ["sns/video_runtime · VideoRuntime", "bundled / new / decode", "wasmi；不启动 Node 或 JS"]),
                     n("guest", "内置供应商 WASM", ["WxIsaac64：仅生成密钥流", "SHA-256 / 燃料 / 内存上限", "CLI 解前 128 KiB 并复制尾部"]),
                     n("mp4", "验证后不覆盖发布", ["ftyp 非完整校验；明文直通", "sync_all / persist_noclobber", "明文 MP4 无需 key 或 WASM"]),
                 ], ["前缀", "密钥流", "流式发布"]),
                 ("06 新注册入口的现状边界（并列能力，不表示彼此调用）", [
                     n("mcp", "wx mcp：17 tools", ["daemon 仅 prepare 语音", "host WAV / 显式 ASR + cache", "旧语义未全覆；无在途取消"]),
                     n("delta", "export-delta-native", ["CLI → ExportDelta → raw UID", "默认新 root / --append-run", "独占新 run；不改原完整输出"]),
                     n("plan", "chat-plan-native", ["estimate / scan 完整 12 列 CSV", "只读显式目录与库清单", "已修复解密根祖先 junction"]),
                     n("historylive", "历史查询已接入", ["CLI / MCP → History → q_history", "history_selection：多类型选页", "全分片分页；复用正文映射"]),
                 ], []),
                 ("07 已接线与验收边界（源码可达不等于全部旧语义兼容）", [
                     n("unwired", "图片验证边界", ["真实进程 / 合成加密账号通过", "用户私有账号未访问、未验证", "响应失败不回滚；后续待验"], "target"),
                     n("mcpvoice", "MCP 语音新路径", ["receipt精确username脱源命中", "显示名可能仍需ResolveChat", "真实模型与新全量待验"], "target"),
                     n("append", "Delta 追加语义上限", ["仅 root / deltas 目录可复用", "同名 run 即使空目录也拒绝", "不是旧批次恢复或全量合并"], "target"),
                     n("cleanup", "最终整体精简门槛", ["先完成全部功能与兼容回归", "再删除无调用与重复模块", "不是当前已完成架构"], "target"),
                 ], []),
             ], notes=["相册已原生；旧 export-sns timeline upsert、transcribe、旧 MCP 与其他 run/GUI 仍保留兼容边界。",
                        "快照以 source-evidence.json 为界；后续公共注册或工具变化须重新核对，不自动升级图中状态。"]),
        dict(name="mcp-voice-flow", title="MCP 语音：后台准备与宿主执行", height=2220,
             subtitle="17 工具已接线，不等于旧语义全迁移 | 内部音频对象不对 MCP 调用方公开",
             rows=[
                 ("01 prepare / bind：宿主策略先于账号访问；initialize 与 tools/list 不走此路径", [
                     n("request", "语音工具参数", ["decode_voice / transcribe_voice", "chat_name + 媒体 local_id", "拒绝工具级路径 / 密钥 / 后端"]),
                     n("policy", "host prepare", ["路径先拒绝 .. 再 absolute", "本地默认：私有 TempDir", "显式后端；无自动云回退"]),
                     n("binding", "固定账号与守卫", ["Pending::bind / RuntimeContext", "先try_cached：详见下行", "配置变化或身份不符即拒绝"]),
                     n("prepareipc", "daemon 仅准备", ["mcp_audio::q_prepare_voice", "唯一关联；SILK + 证据", "不解码 / 不识别 / 不发布"]),
                 ], ["参数", "绑定", "未命中"]),
                 ("02 receipt 快路径：转录 + 显式缓存，语音 IPC 之前；显示名可能仍需 ResolveChat", [
                     n("receiptid", "精确身份只读查询", ["try_cached → lookup_success", "username / media_id / runtime", "授权先检；不使用历史别名"]),
                     n("receiptproof", "历史成功记录核验", ["后端配置 / 证据 / 记录摘要", "Conflict / Miss 不冒充成功", "receipt不是签名或现存证明"]),
                     n("receipthit", "命中直接返回", ["预算 / 原守卫 / context / 账号", "不读已删除源 / 不启动后端", "daemon可停止；缓存字节不变"]),
                     n("receipttest", "持久命中进程证据", ["移走合成消息源并stop daemon", "原MCP与重启MCP仍相同文本", "后端一次；7项进程测试通过"]),
                 ], ["身份", "命中", "验证"]),
                 ("03 未命中：daemon → host；内部 IPC 与公开 MCP 是两个独立预算", [
                     n("payload", "内部 prepared_audio", ["音频 16 MiB；IPC 24 MiB", "外壳预留 1024 bytes", "版本 / Base64 / SHA-256"]),
                     n("hostread", "host finish 复核", ["有界解析 / SHA256 / SILK", "证据 media_local_id 必须匹配", "失败字段先判 QueryFailed"]),
                     n("budget", "独立 MCP 响应预算", ["check_text_result：真实请求 ID", "JSON 转义 + content / isError", "默认 1 MiB；不是内部24MiB"]),
                     n("route", "host 分支执行", ["Decode → 下方04；ASR → 05", "IPC后 remaining 收紧后端", "不将 prepared_audio 返回工具"]),
                 ], ["IPC字节", "上下文", "分支"]),
                 ("04 decode_voice：实际解码 → 暂存 → 真实文本预算与账号复核 → 不覆盖发布", [
                     n("silk", "原生 SILK 解码", ["共享 audio / asr WAV 校验", "24kHz 单声道 PCM16 WAV", "不是伪造的音频描述符"]),
                     n("wavstage", "guarded publish 暂存", ["WAV SHA256.wav 文件名", "暂存 / sync / 身份与字节校验", "共享 HostOutputGuard"]),
                     n("precommit", "before_commit 回调", ["构造旧式纯文本成功模板", "check_text_result / deadline", "绑定账号 / 暂存 / 守卫复核"]),
                     n("wavpublish", "WAV 不覆盖提交", ["persist_noclobber", "提交后不做可失败文件检查", "随后通道断开不能回滚"]),
                 ], ["WAV", "预检", "提交"]),
                 ("05 transcribe_voice：host 显式后端，可选成功缓存；不写永久 WAV", [
                     n("asrauth", "显式 BackendArgs", ["Local：显式 whisper 程序/模型", "Cloud：显式端点/凭证/授权", "默认临时目录由 Pending 清理"]),
                     n("asrcache", "绑定账号成功缓存", ["voice-cache-file 显式启用", "消息 / 音频 / 模型 / 识别配置", "timeout 不参与 identity"]),
                     n("asrrun", "未命中才识别", ["Local 或显式 OpenAI 客户端", "min配置期限与当前remaining", "checked-cache：细节见06"]),
                     n("asrtext", "纯文本结果", ["[本机时间] (language) + text", "独立 MCP 预算与账号检查", "内部音频 / cache对象不公开"]),
                 ], ["授权", "未命中", "文本"]),
                 ("06 checked-cache 与 receipt：识别后预检 + 暂存/快照后、实际 persist 前再检查", [
                     n("cachepre", "识别后 preflight", ["with_receipt_checked 入口", "完整成功文本与真实请求 ID", "预算 / 原始守卫 / context"]),
                     n("cachestage", "缓存暂存与快照", ["共享 cache::store_checked", "write / flush / sync_all", "复核原目标快照与身份"]),
                     n("cachefinal", "actual persist 前回调", ["再次核验预算 / 守卫 / context", "账号 before_commit 拒绝", "不发布本次缓存；保留错误"]),
                     n("cachecommit", "允许才提交缓存", ["persist 或 persist_noclobber", "普通I/O失败仍保留识别成功", "提交后通道断开不回滚"]),
                 ], ["通过", "复核", "允许"]),
                 ("07 兼容与副作用边界：并列说明，不表示可自动回退或回滚", [
                     n("oldcache", "receipt 补索引边界", ["无源弱缓存不能补造身份", "有源强缓存验证证据后可补索引", "精确身份命中不证明源现存"], "target"),
                     n("delivery", "发布不等于送达", ["图片发布后响应失败仍非事务", "WAV预算预检不保证通道可用", "不承诺ACK回滚或恰好一次"], "target"),
                     n("quality", "图片元数据与待验收", ["资源md5 / 加密DAT size + 状态", "非明文摘要/大小；2进程通过", "真实模型 / 私有账号未验"], "target"),
                     n("tests", "历史状态／表情快照", ["1001通过 / 10忽略 / 2警告", "图片2项 / 语音7项；历史结果", "不是本轮SNS最终全量"], "target"),
                 ], []),
             ], notes=["语音prepare对象仅内部传递；模型/云端后端运行于host，不在daemon。",
                        "WAV与缓存均在提交前核验完整文本；任何已提交结果均不承诺通道失败回滚。"]),
    ]


def render(spec, stamp, evidence_pending=False):
    width, height = 1640, spec["height"]
    lines = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
             '<style>text { font-family: "Microsoft YaHei", Arial, sans-serif; letter-spacing:0; }</style>',
             '<defs>']
    for state, (_, color, _) in PALETTE.items():
        lines.append(f'<marker id="arrow-{state}" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0 0 L8 4 L0 8 Z" fill="{color}"/></marker>')
    lines.extend(['</defs>', f'<rect data-graph-role="background" width="{width}" height="{height}" fill="#fff"/>'])
    def text(x, y, content, size=18, color="#111827", owner="", bold=False):
        lines.append(f'<text x="{x}" y="{y}" font-size="{size}" fill="{color}" font-weight="{600 if bold else 400}" data-owner="{owner}">{escape(content)}</text>')
    text(60, 54, spec["title"], 32, bold=True)
    text(60, 89, spec["subtitle"], 18, "#4b5563")
    evidence_label = "证据待封存；现有 source-evidence.json 为旧快照" if evidence_pending else "证据：source-evidence.json；不代表已安装版本"
    text(60, 119, f"源码核对快照：{stamp} | {evidence_label}", 16, "#4b5563")
    boxes = {}
    for row_index, (heading, nodes, _) in enumerate(spec["rows"]):
        y = 220 + row_index * 270
        text(60, y - 27, heading, 20, bold=True)
        for index, item in enumerate(nodes):
            boxes[item["id"]] = (60 + index * 400, y, 320, 166)

    def edge(source, target, label, state, points=None):
        sx, sy, sw, sh = boxes[source]
        tx, ty, tw, th = boxes[target]
        if points:
            route = [(sx + sw / 2, sy), *points, (tx + tw / 2, ty + th)]
        else:
            route = [(sx + sw, sy + sh / 2), (tx, ty + th / 2)]
        color = PALETTE[state][1]
        dash = 'stroke-dasharray="8 6"' if state in ("pending", "target") else ''
        d = 'M ' + ' L '.join(f'{x} {y}' for x, y in route)
        lines.append(f'<path data-graph-role="edge" data-source="{source}" data-target="{target}" d="{d}" fill="none" stroke="{color}" stroke-width="2" {dash} marker-end="url(#arrow-{state})"/>')
        if label:
            if points:
                text(1120, 408, label, 16, color)
            else:
                text(sx + sw + 6, sy + sh / 2 - 12, label, 16, color)

    for _, nodes, labels in spec["rows"]:
        for index, label in enumerate(labels):
            if label is not None:
                edge(nodes[index]["id"], nodes[index + 1]["id"], label, nodes[index]["state"])
    for source, target, points, label, state in spec.get("extra", []):
        edge(source, target, label, state, points)
    for source, target in spec.get("continuations", []):
        sx, sy, sw, sh = boxes[source]
        tx, ty, _, th = boxes[target]
        # 跨行走标题左侧的外部通道，保留完整调用连线且不穿过节点。
        d = f'M {sx + sw / 2} {sy + sh} V {sy + sh + 20} H 36 V {ty + th / 2} H {tx}'
        lines.append(f'<path data-graph-role="edge" data-source="{source}" data-target="{target}" d="{d}" fill="none" stroke="#15803d" stroke-width="2" marker-end="url(#arrow-live)"/>')
    for _, nodes, _ in spec["rows"]:
        for item in nodes:
            x, y, w, h = boxes[item["id"]]
            fill, color, status = PALETTE[item["state"]]
            lines.append(f'<rect id="{item["id"]}" data-graph-role="node" x="{x}" y="{y}" width="{w}" height="{h}" rx="8" fill="{fill}" stroke="{color}"/>')
            text(x + 16, y + 29, item["title"], 21, owner=item["id"], bold=True)
            text(x + 16, y + 54, status, 14, color, item["id"])
            for i, label in enumerate(item["lines"]):
                text(x + 16, y + 85 + i * 29, label, 17, owner=item["id"])
    for i, (state, (_, color, label)) in enumerate(PALETTE.items()):
        x, y = 60 + i * 400, height - 105
        lines.append(f'<rect data-graph-role="legend" x="{x}" y="{y-14}" width="14" height="14" fill="{color}"/>')
        text(x + 24, y, label, 17, color)
    for i, note in enumerate(spec["notes"]):
        text(60, height - 64 + i * 27, note, 17, "#4b5563")
    lines.append('</svg>')
    (HERE / (spec["name"] + '.svg')).write_text('\n'.join(lines) + '\n', encoding='utf-8')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check-source', action='store_true')
    parser.add_argument('--preserve-evidence', action='store_true', help='仅文案重绘，保留原源码核对快照')
    parser.add_argument('--defer-evidence', action='store_true', help='并行源码仍在更新：校验并绘图，暂不封存证据')
    parser.add_argument('--diagram', choices=[spec['name'] for spec in diagrams()], help='仅更新指定现有图')
    parser.add_argument('--skill-root', type=pathlib.Path, default=pathlib.Path.home() / '.codex/skills/fireworks-tech-graph')
    args = parser.parse_args()
    evidence = HERE / 'source-evidence.json'
    if args.check_source:
        saved = json.loads(evidence.read_text(encoding='utf-8'))
        changed = [name for name, digest in hashes().items() if saved['sha256'].get(name) != digest]
        if changed:
            sys.exit('STALE: source changed; re-audit diagram claims before regeneration: ' + ', '.join(changed))
        print('source evidence: unchanged')
        return
    selected = [spec for spec in diagrams() if not args.diagram or spec['name'] == args.diagram]
    stamp = (json.loads(evidence.read_text(encoding='utf-8'))['snapshot']
             if args.preserve_evidence else selected[0].get('snapshot', SNAPSHOT))
    before = hashes()
    anchors = verify_wiring()
    for spec in selected:
        render(spec, stamp if args.preserve_evidence else spec.get('snapshot', SNAPSHOT), args.defer_evidence)
        for check in ['xml', 'markers', 'collisions', 'geometry', 'composition']:
            cmd = [sys.executable, str(args.skill_root / 'scripts/validate_svg.py'), str(HERE / (spec['name'] + '.svg')), '--check', check]
            print(subprocess.list2cmdline(cmd), flush=True)
            subprocess.run(cmd, check=True)
        print(spec['name'] + ': Fireworks checks passed', flush=True)
    if hashes() != before and not args.defer_evidence:
        sys.exit('Source changed during generation; rerun after reviewing changes')
    if args.defer_evidence:
        print('source evidence: deferred; final source review and snapshot still required')
    elif args.preserve_evidence:
        print('source evidence: preserved original audit snapshot')
    else:
        evidence.write_text(json.dumps({
            'snapshot': stamp, 'captured_utc': datetime.now(timezone.utc).isoformat(), 'sha256': before,
            'release_evidence_status': 'sns_native_album_1121_stage_regression_passed_not_full_migration',
            'stage_validation': {
                'targets': 19, 'passed_executions': 1121, 'failed': 0, 'ignored': 11,
                'counts_are_unique_features': False, 'album_cli': {'passed': 12, 'failed': 0},
                'msvc_check_unused_warnings': 10, 'exit_codes_confirmed_by_main': 0,
                'rustfmt_check': 'passed, confirmed by main; not rerun by documentation worker',
                'logs': {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in [
                    pathlib.Path('C:/CodexLocal/wx-cli-album-complete-final-tests.log'),
                    pathlib.Path('C:/CodexLocal/wx-cli-album-complete-final-check.log'),
                ]},
            },
            'rendered_diagrams': [spec['name'] for spec in selected],
            'wiring_anchors': anchors, 'mcp_registered_tools': MCP_TOOLS,
            'snapshot_boundaries': [
                'SNS media wiring update: CLI --download-media explicitly creates DownloadOptions; None remains offline. export_database_with_media reuses the existing per-contact staging, cache-first recovery, guarded download for unresolved media, shared JSON/HTML references and fresh-directory rename publication. Existing SNS destinations are rejected; no upsert.',
                'Native sns_album calls album::export with one fixed RuntimeContext. Feed performs at most six read attempts, each 30 seconds and 256 MiB, with backoff; not a whole-batch deadline. q_sns_feed returns resolved_user, scanned and scan_truncated; missing exact author is rejected and truncation warns.',
                'album calls album_images, album_videos, album_render and publish. Images: existing then url then thumb, image_cache=0. Videos: existing then complete cache then remote then partial cache. The CLI builds an account video-only cache index only when videos are enabled/present and cache exists. Per-worker WASM is lazy; plaintext does not load it.',
                'OutputTree prepare pins/protects inputs, validates candidate paths and source binding, then album stages in a sibling directory. Publication replaces media, timeline array JSON, HTML and summary atomically per file, not transactionally across the tree. Earlier commits survive failure; unknown old files are not deleted. Explicit adopt marks legacy_unverified and never authenticates old media provenance; known identity conflicts are rejected.',
                'Bounded legacy JSON and manifest reads, existing video_bytes, final-copy mtime preservation and h1 overflow-wrap:anywhere are present in audited source. Latest full-suite log: C:/CodexLocal/wx-cli-album-complete-final-tests.log, 19 targets, 1121 passed executions, 0 failed, 11 ignored; main confirmed exit 0. Album CLI 12/0 includes cache mtime at line 1292 and encrypted video at line 1298. Executions include repeated modules, not unique features. Earlier 107/0/2 and first-full 1119/0/11 are superseded stage snapshots.',
                'Final MSVC check: C:/CodexLocal/wx-cli-album-complete-final-check.log, main confirmed exit 0, 10 unused warnings. Scoped rustfmt check passed per main. Long h1 adds only overflow-wrap:anywhere; the browser fixture uses an empty video placeholder with one expected resource error, not successful playback evidence. No cargo was run for this documentation pass. No installed-version, private-account, real-model or complete migration acceptance is implied.',
                'toolkit export-sns still uses Python export_sns.py timeline upsert; export_all/transcribe/other run/GUI gaps remain. The old album script is retained as legacy material but is not the native sns-album production route. No claim of complete Rust migration.',
                'Historical SNS media + lifecycle stage, NOT current album acceptance: main confirmed MSVC cargo test, single-threaded, EXIT 0; C:/CodexLocal/wx-cli-sns-lifecycle-final-tests.log. The 18 target result groups total 1061 passed executions, 0 failed, 11 ignored; main unit target 698/0/7. Repeated production-module test executions are included; these are not 1061 independent features or unique tests. Earlier download/socket and stale-PID lifecycle failures were resolved before that historical run.',
                'MSVC cargo check EXIT 0 confirmed by main; C:/CodexLocal/wx-cli-sns-lifecycle-final-check.log. Ordinary binary/check report 28 unused/build warnings, including not-yet-wired foundations. The full test log also reports 2 test-binary warnings, 1 duplicate; not warning-free. Prior lifecycle targeted evidence was 5/5 repeated passes and runtime_isolation 12/0/2. This is stage regression evidence, not full migration, installed-version, real-account or release acceptance; older 1001-stage statistics below remain historical.',
                'This pass updates only rendered_diagrams. Other retained diagrams are earlier snapshots, not newly reviewed SNS media diagrams. Python is a documentation-generation tool, not a product runtime requirement.',
                '17 MCP tools are source-registered: 14 read-only tools plus decode_image, decode_voice and transcribe_voice. This is not complete legacy semantic parity; no in-flight stdio cancellation.',
                'decode_image requires an existing host media-output-root; image-key-file is host-only. No overwrite, upload, download or automatic key discovery. Source wiring is not successful integration evidence.',
                'Image publication and IPC/MCP delivery are not one transaction: timeout, response limits or delivery failure can follow publication without rollback. No automatic retry, idempotent-success or exactly-once promise.',
                'MCP uses explicit WX_CLI_CONFIG with a pinned configuration and bounded IPC; daemon startup has its existing independent limit.',
                'Delta default creates a new root; --append-run calls create_run_in_existing_root and exclusively creates deltas/run_id. Existing full outputs and existing runs are not modified or resumed.',
                'Plan estimate/scan keeps the original 12 CSV fields; decrypted-root ancestors are checked before canonicalization and SQLite.',
                'toolkit transcribe-database-native resolves an exact message source/table/local_id to unique VoiceInfo via username/server_id/time, then passes SILK bytes into existing ASR. No temporary SILK; local backend still uses controlled temporary WAV.',
                'Database ASR evidence explicitly sets account_authenticated=false; caller-supplied static snapshot provenance is not account authentication. No database writeback or raw audio in output.',
                'mcp_refer and mcp_attachments share strict_message. File/record references use account-bound attachment_refs without media decode, upload or attachment writes.',
                'ASR cache/cached are reached by explicit database CLI flags and MCP host voice-cache-file. Execution timeout no longer affects local successful cache identity; old digests remain stored but do not immediately hit.',
                'mcp_audio is registered and daemon only prepares bounded SILK/evidence. Internal voice IPC is 24 MiB, raw audio 16 MiB; public MCP frame budget remains independent.',
                'Host validates prepared_audio and media ID, uses private default TempDir and explicit local/cloud backend, and clamps backend timeout after IPC. No automatic cloud fallback or model download.',
                'WAV publication checks exact text with the real request ID before no-clobber commit. Pure text templates are returned, not prepared audio. A later channel failure cannot roll back published files.',
                'Checked-cache all-targets log reviewed: 909 passed executions in 14 groups, 0 failed, 9 ignored, 1 unused store_success compatibility API warning. Main confirms MSVC success with the same warning. Earlier ASR 106 passed is a separate pre-enhancement snapshot.',
                'Host uses transcribe_cached_with_receipt_checked preflight and shared cache checked publication after staging/snapshot verification, before actual persist. Exact text budget, original guards, context and account rejection prevent this commit; ordinary I/O failure remains recoverable. Later channel failure does not roll back committed results.',
                'Independent cache fixture: 157 passed, 3 ignored, 14 library and 5 test warnings; protocol 34 passed with 5 lint warnings. Do not claim every fixture warning-free.',
                'Image directory-swap independent regression is fixed: main confirmed 20 passed, 2 ignored. Image publication and delivery remain nontransactional. Darwin host audit continues.',
                'Receipt fast path is wired before voice IPC: exact username/media ID, account, backend identity and record digest are required. Hit is read-only and can survive deleted sources and stopped daemon; display names may still require ResolveChat. Receipt is not a signature or proof of current source existence.',
                'A weak cache without source evidence cannot invent a receipt. An existing strong successful cache can gain a receipt after real source identity/evidence validation, without repeating recognition.',
                'Historical metadata-receipt snapshot: 962 passed executions across 14 groups, 0 failed, 9 ignored; MSVC check passed with 2 unused-code warnings. Superseded by the status-emoticons baseline, not erased or relabeled as a current full-suite result.',
                'Status/emoticons-stage all-targets: C:/CodexLocal/wx-cli-status-emoticons-final-tests.log; main confirmed exit 0; 16 result groups total 1001 passed, 0 failed, 10 ignored, 2 unchanged ASR warnings. Counts are executions, not unique tests. MSVC check and scoped code git diff --check exit 0 confirmed by main for that stage; C:/CodexLocal/wx-cli-status-emoticons-final-check.log.',
                'After that tested stage, prepare_emoticons was renamed to shared toolkit_run_prepare::prepare with the same emoticons behavior, per main. Source hashes and anchors reflect this rename and may include new untested run-decrypt work; the 1001 baseline does not validate that newer code. No run-decrypt migration or acceptance is depicted by this update.',
                'run status / -s uses run_status::inspect for read-only config-relative key metadata, DB/export byte totals and messages/chats truthiness; unreadable transcripts warn. Existing toolkit status environment report is unchanged.',
                'Emoticons: CLI prepare checks configured process name; saved keys are retained without whole-database HMAC. Newly scanned keys are validated then atomically saved; unsafe paths are rejected. Offline export uses saved keys. Both enter isolated DbCache cache/emoticons, catalog, filter/preview or guarded HTTP/AES download and optional ffmpeg HEVC conversion. Images publish without clobber; bin fallback is replaceable; per-item failures retain batch exit 0.',
                'toolkit_run_prepare: 8 unit tests passed. All 6 emoticons runtime tests passed, including loopback HTTP and synthetic process-name run-emoticons saved-key reuse; no real process memory scanning was tested.',
                'Additional explicit FFmpeg test: C:/CodexLocal/wx-cli-emoticons-ffmpeg-test.log; 1 passed, 0 failed, separate from the ordinary 1001/0/10 baseline. Do not relabel as 1002/0/9.',
                'get_chat_images now requests image_metadata and projects md5/size/resource_status/size_status/size_kind/binding. Size is encrypted DAT metadata, not plaintext image size; missing/ambiguous states remain explicit. Real model quality and full G01-G12 parity remain unverified.',
                'SNS cache restoration and ASR transcription caching are independent paths.',
                'Source reachability is not installed-version status, a full migration claim, or a release-quality certification.',
                'Parallel changes after captured_utc require re-audit; --check-source fails on tracked source drift.',
            ],
        }, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
