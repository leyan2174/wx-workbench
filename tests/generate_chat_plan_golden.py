"""Only synthetic SQLite; AST-load pure legacy functions without importing its app/config."""
import ast
import contextlib
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
from datetime import datetime

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "vendor/wechat-decrypt/export_all_chats.py"
FUNCTIONS = {
    "_where_for_time_range", "_query_message_table_plan_stats",
    "_query_message_table_plan_stats_conn", "_new_plan_accumulator",
    "_message_table_name_for_username", "_fetch_existing_message_tables",
    "_collect_message_stats_batch", "_collect_resource_estimates_batch",
    "_collect_voice_estimates_batch", "_finalize_plan_stats", "_format_plan_time",
    "_scan_dir_bytes", "_scan_local_attachment_bytes",
}


def generate():
    source = ast.parse(SOURCE.read_text(encoding="utf-8-sig"))
    module = ast.Module(body=[n for n in source.body if isinstance(n, ast.FunctionDef) and n.name in FUNCTIONS], type_ignores=[])
    env = dict(sqlite3=sqlite3, closing=contextlib.closing, hashlib=hashlib, datetime=datetime, os=os)
    env["mcp_server"] = SimpleNamespace(_is_safe_msg_table_name=lambda t: t.startswith("Msg_") and len(t) == 36 and all(c in "0123456789abcdef" for c in t[4:]))
    exec(compile(module, str(SOURCE), "exec"), env)
    table = "Msg_" + hashlib.md5(b"alpha").hexdigest()
    sql = {
        "messages.db": f"""
CREATE TABLE [{table}] (create_time INTEGER, message_content, compress_content, packed_info_data);
INSERT INTO [{table}] VALUES (NULL,NULL,NULL,NULL),(0,'zero',NULL,NULL),(99,'before',NULL,NULL),(100,'中文',x'010203',NULL),(101,NULL,NULL,NULL),(102,'abc',NULL,x'00ff'),(103,'after',NULL,NULL);
CREATE TABLE [Msg_{hashlib.md5(b'broken').hexdigest()}] (wrong INTEGER);
CREATE TABLE [Msg_{hashlib.md5(b'empty').hexdigest()}] (create_time INTEGER, message_content, compress_content, packed_info_data);
""",
        "resource.db": """
CREATE TABLE ChatName2Id(user_name TEXT);
INSERT INTO ChatName2Id VALUES ('alpha');
CREATE TABLE MessageResourceInfo(chat_id INTEGER, message_id INTEGER, message_create_time INTEGER);
CREATE TABLE MessageResourceDetail(message_id INTEGER,size INTEGER);
INSERT INTO MessageResourceInfo VALUES (1,1,100),(1,2,102),(1,3,103),(1,4,101);
INSERT INTO MessageResourceDetail VALUES (1,12),(1,NULL),(2,30),(3,900);
""",
        "media.db": """
CREATE TABLE Name2Id(user_name TEXT);
INSERT INTO Name2Id VALUES ('alpha');
CREATE TABLE VoiceInfo(chat_name_id INTEGER,create_time INTEGER,voice_data BLOB);
INSERT INTO VoiceInfo VALUES (1,100,x'0102'),(1,102,x'010203'),(1,101,NULL),(1,103,x'00000000');
""",
        "bad.db": "CREATE TABLE unrelated(value INTEGER);",
    }
    scenarios = [
        dict(name="bounded", message=["messages.db"], resource="resource.db", media=["media.db"], start=100, end=102),
        dict(name="unbounded", message=["messages.db"], resource="resource.db", media=["media.db"], start=None, end=None),
        dict(name="empty_range", message=["messages.db"], resource=None, media=[], start=200, end=200),
        dict(name="missing", message=[], resource=None, media=[], start=None, end=None),
        dict(name="broken_resources", message=["messages.db"], resource="bad.db", media=["media.db", "bad.db", "media.db"], start=100, end=102),
        dict(name="start_only", message=["messages.db"], resource="resource.db", media=["media.db"], start=102, end=None),
        dict(name="end_only", message=["messages.db"], resource="resource.db", media=["media.db"], start=None, end=100),
    ]
    # 最后一片必须不同路径，以验证出错后不会继续累计。
    sql["media_later.db"] = sql["media.db"]
    scenarios[4]["media"][-1] = "media_later.db"
    users = ["alpha", "empty", "broken", "absent", "x' OR 1=1 --"]
    with tempfile.TemporaryDirectory(prefix="wx-plan-synthetic-") as directory:
        root = Path(directory)
        for name, script in sql.items():
            with contextlib.closing(sqlite3.connect(root / name)) as conn:
                conn.executescript(script)
        for scenario in scenarios:
            env["_iter_message_db_paths"] = lambda: [str(root / p) for p in scenario["message"]]
            env["_get_message_resource_db_path"] = lambda: str(root / scenario["resource"]) if scenario["resource"] else None
            env["mcp_server"]._iter_media_db_paths = lambda: [str(root / p) for p in scenario["media"]]
            start, end = scenario["start"], scenario["end"]
            acc = env["_collect_message_stats_batch"](users, start, end)
            for fn in ["_collect_resource_estimates_batch", "_collect_voice_estimates_batch"]:
                values, statuses = env[fn](users, start, end)
                for user in users:
                    acc[user]["attachment_estimated_bytes"] += values[user]
                    acc[user]["statuses"].update(statuses[user])
            scenario["expected"] = {}
            for user in users:
                result = env["_finalize_plan_stats"](acc[user])
                result.pop("first_time")
                result.pop("last_time")
                result["first_ts"] = acc[user]["first_ts"]
                result["last_ts"] = acc[user]["last_ts"]
                result["attachment_scanned_bytes"] = None
                scenario["expected"][user] = result
        direct = []
        for start, end in [(None,None),(100,102),(200,200),(0,0)]:
            direct.append(dict(start=start,end=end,values=list(env["_query_message_table_plan_stats"](str(root / "messages.db"),table,start,end))))
        # 文件内容全部合成；同名联系人只按 username 哈希归属。
        alpha = hashlib.md5(b"alpha").hexdigest()
        empty = hashlib.md5(b"empty").hexdigest()
        files = {
            f"msg/attach/{alpha}/a.bin": 13,
            f"msg/attach/{alpha}/nested/copy.bin": 13,
            f"msg/file/{alpha}/document": 7,
            f"msg/video/{alpha}/clip": 11,
            f"msg/attach/{empty}/a.bin": 5,
            "msg/file/unattributed.bin": 1000,
            "cache/ignored.bin": 2000,
        }
        source_dir = root / "source"
        for name, size in files.items():
            path = source_dir / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"s" * size)
        links = {f"msg/attach/{alpha}/hard.bin": f"msg/attach/{alpha}/a.bin"}
        for name, target in links.items():
            os.link(source_dir / target, source_dir / name)
        env["mcp_server"].WECHAT_BASE_DIR = str(source_dir)
        scan = dict(files=files, hardlinks=links, expected={user: list(env["_scan_local_attachment_bytes"](user)) for user in users})
        env["mcp_server"].WECHAT_BASE_DIR = ""
        scan["missing_base"] = list(env["_scan_local_attachment_bytes"]("alpha"))
    fields = next(ast.literal_eval(n.value) for n in source.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id == "PLAN_CSV_FIELDS" for t in n.targets))
    stream = io.StringIO(newline="")
    writer = csv.writer(stream)
    writer.writerow(fields)
    writer.writerow(["",1,"alpha",'中文, "quoted"\r\nline',"single",0,"","",0,"",0,"ok"])
    return dict(sql=sql, users=users, scenarios=scenarios, direct=direct, csv="\ufeff"+stream.getvalue(), scan=scan)


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    path = ROOT / "tests/fixtures/chat-plan-golden.json"
    result = generate()
    if args.check:
        assert json.loads(path.read_text(encoding="utf-8")) == result, "golden differs from legacy"
        print("PASS: synthetic SQLite golden matches legacy Python")
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(result, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
        print(f"Generated {path}: {len(result['scenarios'])} scenarios")
