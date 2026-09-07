"""仅合成数据：隔离执行旧 delta 函数，生成契约 golden；可运行临时 Rust harness。"""
import argparse
import ast
from contextlib import closing
from datetime import datetime, timedelta, timezone
import hashlib
import json
import ntpath
import os
from pathlib import Path
import re
import subprocess
import tempfile
from types import SimpleNamespace

REPO = Path(__file__).resolve().parents[1]
FIXTURE = REPO / "tests/fixtures/chat-delta-golden.json"


def generate():
    source = REPO / "vendor/wechat-decrypt/export_all_chats.py"
    selected = {"_safe_export_filename_part", "_delta_filename", "_content_hash_for_uid",
                "_delta_msg_uid", "_date_from_message_ts", "export_delta_one", "_write_delta_manifest"}
    tree = ast.parse(source.read_text(encoding="utf-8"))
    tree.body = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in selected]

    class Clock:
        @staticmethod
        def now():
            return datetime(2026, 9, 7, 12, 0, 0)

        @staticmethod
        def fromtimestamp(ts):
            return datetime.fromtimestamp(ts, timezone(timedelta(hours=8)))

    window = dict(start=100, end=200, run_id="synthetic-run", utc_offset_seconds=28800,
                  generated_at="2026-09-07 12:00:00")
    def message(lid, ts, raw, kind="text", rendered="rendered", extras=None, db="C:\\synthetic\\message_0.db"):
        return dict(db_path=db, local_id=lid, timestamp=ts, sender="synthetic sender", msg_type=kind,
                    raw_content=raw, rendered=rendered, extras=extras or {})
    text = lambda s: dict(kind="text", value=s)
    rows = [message(4, 201, text("excluded end")), message(1, 100, text("中文|原文"), rendered="中文"),
            message(2, 200, dict(kind="null"), rendered=None),
            message(3, 150, dict(kind="bytes", value=[0, 39, 10, 255, 92]), "voice", None,
                    {"type": "audio", "transcription": "synthetic transcript", "duration": 3}),
            message(5, 99, text("excluded start")),
            message(6, 150, text(""), "image", None, {"image": "synthetic.jpg"}),
            message(7, 151, dict(kind="bytes", value=list(range(256))), "text"),
            message(8, 152, dict(kind="python_repr", value="-42"), "text", "-42")]
    rows.append(dict(rows[1]))
    chats = [dict(username="synthetic_user", display_name=" 合成/测试:*? ", is_group=False, messages=rows),
             dict(username="synthetic@chatroom", display_name="", is_group=True,
                  messages=[message(9, 100, text("group"), db="D:message_1.db")]),
             dict(username="empty", display_name="Empty", is_group=False, messages=[]),
             dict(username="failed", display_name="Failed", is_group=False, messages=[], source_error="no tables")]
    current = {}
    def raw_value(raw):
        if raw["kind"] == "null": return None
        if raw["kind"] == "bytes": return bytes(raw["value"])
        if raw["kind"] == "python_repr": return int(raw["value"])
        return raw["value"]
    def context(username):
        chat = current["chat"]
        tables = [] if chat.get("source_error") else [dict(db_path=db, table_name="synthetic")
                   for db in dict.fromkeys(m["db_path"] for m in chat["messages"])]
        if not tables and not chat.get("source_error"):
            tables = [dict(db_path="empty.db", table_name="synthetic")]
        return dict(display_name=chat["display_name"], is_group=chat["is_group"], message_tables=tables)
    class Connection:
        def __init__(self, path): self.path = path
        def close(self): pass
    def query(conn, table, start_ts, end_ts, **kw):
        return [(m["local_id"], m["msg_type"], m["timestamp"], 0, raw_value(m["raw_content"]), None)
                for m in sorted(current["chat"]["messages"], key=lambda m: m["timestamp"])
                if m["db_path"] == conn.path and m["timestamp"] >= start_ts
                and (end_ts is None or m["timestamp"] <= end_ts)]
    def extract(lid, *args):
        m = next(m for m in current["chat"]["messages"] if m["local_id"] == lid)
        return m["rendered"], m["extras"]
    ns = dict(os=SimpleNamespace(path=ntpath, makedirs=os.makedirs), hashlib=hashlib, re=re,
              json=json, datetime=Clock, Path=Path, closing=closing, DELTA_SCHEMA_VERSION=1,
              _UNSAFE_FILENAME_RE=re.compile(r'[\\/:*?"<>|]'), sqlite3=SimpleNamespace(connect=Connection),
              mcp_server=SimpleNamespace(_resolve_chat_context=context, _load_name2id_maps=lambda _: {}, _query_messages=query),
              _resolve_sender=lambda *args: "synthetic sender", _msg_type_str=lambda t: t, _extract_content=extract,
              _contact_metadata_for_export=lambda *args: dict(contact_remark="", contact_nick_name="", contact_tags=[], contact_memo=""))
    exec(compile(tree, str(source), "exec"), ns)
    # Windows 路径语义是目标契约；oracle 运行环境也必须为 Windows。
    if os.name != "nt": raise RuntimeError("golden generation requires Windows")
    cases, results = [], []
    with tempfile.TemporaryDirectory(prefix="wx-delta-oracle-") as root:
        for chat in chats:
            current["chat"] = chat
            result = ns["export_delta_one"](chat["username"], root, {}, window["run_id"], window["start"], window["end"])
            document = None
            if result.get("path"):
                document = json.loads((Path(root) / "deltas" / window["run_id"] / result["path"]).read_text(encoding="utf-8"))
            cases.append(dict(input=chat, result=result, document=document))
            results.append(result)
        path = ns["_write_delta_manifest"](root, window["run_id"], window["start"], window["end"], len(chats), results)
        manifest = json.loads(path.read_text(encoding="utf-8"))
    return dict(window=window, cases=cases, manifest=manifest)


def test_rust():
    with tempfile.TemporaryDirectory(prefix="wx-delta-rust-") as directory:
        root = Path(directory)
        module = (REPO / "src/toolkit/chat_delta.rs").as_posix()
        (root / "lib.rs").write_text(f'#[path = "{module}"]\npub mod chat_delta;\n', encoding="utf-8")
        (root / "Cargo.toml").write_text('''[package]
name = "chat-delta-harness"
version = "0.0.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
anyhow = "1"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "=1.0.140", features = ["arbitrary_precision"] }
sha2 = "0.10"
tempfile = "3"
''', encoding="utf-8")
        env = dict(os.environ, LIBCLANG_PATH=r"C:\CodexLocal\build-tools\libclang\clang\native")
        logs = Path(r"C:\CodexLocal\日志")
        logs.mkdir(parents=True, exist_ok=True)
        for subcommand in ("check", "test"):
            command = ["cargo", subcommand, "--manifest-path", str(root / "Cargo.toml"), "--target", "x86_64-pc-windows-msvc", "--offline"]
            print("COMMAND:", subprocess.list2cmdline(command), flush=True)
            completed = subprocess.run(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace")
            log = logs / f"chat-delta-{subcommand}.log"
            log.write_text(completed.stdout, encoding="utf-8")
            print(completed.stdout, flush=True)
            print(f"EXIT: {completed.returncode}; FULL LOG: {log}", flush=True)
            if completed.returncode: raise SystemExit(completed.returncode)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--test-rust", action="store_true")
    args = parser.parse_args()
    generated = generate()
    if args.check:
        assert json.loads(FIXTURE.read_text(encoding="utf-8")) == generated
        print("PASS: synthetic Python oracle matches golden", flush=True)
    else:
        FIXTURE.parent.mkdir(parents=True, exist_ok=True)
        FIXTURE.write_text(json.dumps(generated, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"WROTE: {FIXTURE}", flush=True)
    if args.test_rust: test_rust()
