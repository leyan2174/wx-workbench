"""合成 SQLite -> 旧 AST delta oracle；不会导入服务或读取用户配置。"""
import argparse
import ast
import base64
from contextlib import closing
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import sqlite3
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LOGS = REPO / "target" / "test-logs"


def execute(command, log, **kwargs):
    print("COMMAND:", subprocess.list2cmdline([str(x) for x in command]), flush=True)
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            encoding="utf-8", errors="replace", **kwargs)
    LOGS.mkdir(parents=True, exist_ok=True)
    target = LOGS / log
    target.write_text(result.stdout, encoding="utf-8")
    print(result.stdout, flush=True)
    print(f"EXIT: {result.returncode}; FULL LOG: {target}", flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)


def generate(zstd):
    extract = runpy.run_path(str(REPO / "tests/generate_export_content_golden.py"))["extract"]
    scope = dict(re=re, ET=ET, base64=base64, sqlite3=sqlite3, hashlib=hashlib,
                 json=json, os=os, Path=Path, closing=closing)
    functions = {
        "_split_msg_type", "_collapse_text", "_parse_xml_root", "_parse_int",
        "_parse_app_message_outer", "_format_app_message_text", "_extract_transfer_info",
        "_format_transfer_message_text", "_format_voip_message_text",
        "_format_record_message_text", "_format_record_dataitem",
        "_format_redpacket_message_text", "_format_finder_message_text",
        "_extract_refer_info", "_summarize_refer_content", "_format_refer_message_text",
        "_resolve_quote_sender_label", "_display_name_for_username",
        "_decompress_content", "_parse_message_content", "_resolve_sender_label",
        "_load_name2id_maps", "_build_message_filters", "_query_messages",
    }
    extract(REPO / "vendor/wechat-decrypt/mcp_server.py", functions, {
        "_XML_PARSE_MAX_LEN", "_XML_UNSAFE_RE", "_RECORD_XML_PARSE_MAX_LEN",
        "_TRANSFER_PAYSUBTYPE_LABEL", "_RECORD_MAX_ITEMS", "_RECORD_MAX_LINE_LEN",
        "_RECORD_DATATYPE_LABEL", "_REFER_INNER_TYPE_LABEL", "_INNER_APPMSG_TYPE_LABEL",
        "_REDPACKET_AMOUNT_SCENES", "_REDPACKET_AMOUNT_RE", "_REDPACKET_SENDER_RE",
    }, scope)
    names = {"wxid_self": "合成自己", "wxid_peer": "合成联系人", "wxid_member": "合成群成员",
             "synthetic@chatroom": "合成群", "wxid_empty": "合成空会话", "wxid_missing": "合成缺表"}
    server = SimpleNamespace(**{name: scope[name] for name in functions})
    server.get_contact_names = lambda: names
    scope["_get_self_username"] = lambda: "wxid_self"
    scope["_zstd_dctx"] = zstd.ZstdDecompressor()
    scope["_is_safe_msg_table_name"] = lambda name: re.fullmatch(r"Msg_[0-9a-f]{32}", name) is not None
    # 发送者函数只用这个入口返回的前缀，正文渲染仍执行真实 _extract_content。
    server._format_message_text = lambda lid, kind, decoded, group, *args: scope["_parse_message_content"](decoded, kind, group)
    scope["mcp_server"] = server
    extract(REPO / "vendor/wechat-decrypt/chat_export_helpers.py", {
        "_extract_content", "_msg_type_str", "_resolve_sender", "_decode_sticker_desc",
        "_format_sticker_message", "_format_system_message", "_format_video_message", "_extract_transfer_extras",
    }, {"MSG_TYPE_MAP"}, scope)
    extract(REPO / "vendor/wechat-decrypt/export_all_chats.py", {
        "export_delta_one", "_delta_msg_uid", "_content_hash_for_uid", "_delta_filename",
        "_safe_export_filename_part", "_date_from_message_ts", "_write_delta_manifest", "_contact_metadata_for_export",
    }, {"DELTA_SCHEMA_VERSION", "_UNSAFE_FILENAME_RE"}, scope)

    class Clock:
        @staticmethod
        def now(): return datetime(2026, 9, 7, 12, 0, 0)

        @staticmethod
        def fromtimestamp(ts): return datetime.fromtimestamp(ts, timezone(timedelta(hours=8)))

    scope["datetime"] = Clock
    contacts = [dict(username=u, remark=names[u], nick_name="合成昵称", description="合成备注")
                for u in ["wxid_peer", "wxid_empty", "wxid_missing"]]
    server.get_contact_full = lambda: contacts
    server.get_contact_tag_names_by_username = lambda: {}
    window = dict(start=100, end=200, run_id="query-oracle", utc_offset_seconds=28800, generated_at="2026-09-07 12:00:00")
    def row(lid, kind, ts, sender, raw, compression=0):
        return dict(local_id=lid, local_type=kind, timestamp=ts, sender_id=sender,
                    raw={"kind": "null"} if raw is None else
                    {"kind": "bytes", "value": list(raw)} if isinstance(raw, bytes) else
                    {"kind": "text", "value": raw}, compression=compression)
    packed = zstd.ZstdCompressor(level=3).compress("wxid_member:\n压缩正文不是摘要".encode())
    peer_rows = [row(99, 1, 99, 2, "窗口前"), row(9, 1, 100, 2, "首端点"),
                 row(2, 1, 100, 1, "同时间保留插入顺序"), row(3, 3, 120, 2, "<msg><img/></msg>"),
                 row(4, 34, 130, 1, None), row(5, 1, 140, 2, b"binary\xff\n"),
                 row(6, 1, 150, 2, "TEXT 标记压缩也不解压", 4), row(7, 1, 160, 2, b"broken zstd", 4),
                 row(8, 43, 180, 2, '<msg><videomsg playlength="5"/></msg>'),
                 row(10, 1, 200, 2, "末端点"), row(11, 1, 201, 2, "窗口后"),
                 row(12, 1, None, 2, "NULL 时间不进入闭区间")]
    group_rows = [row(1, 1, 100, 3, packed, 4),
                  row(2, 49, 150, 1, 'wxid_member:<msg><appmsg><type>5</type><title>合成链接</title></appmsg></msg>'),
                  row(3, (57 << 32) | 49, 160, 4, 'wxid_member:\n<msg><appmsg><type>57</type><title>回复原文</title><refermsg><type>1</type><fromusr>wxid_self</fromusr><displayname>合成自己</displayname><content>被引用原文</content></refermsg></appmsg></msg>'),
                  row(4, 99, 170, None, b"wxid_member:\nunknown"),
                  row(6, 49, 190, 1, 'wxid_self:\n<msg><appmsg><type>2000</type><title>合成转账</title><wcpayinfo><paysubtype>1</paysubtype><feedesc>¥0.01</feedesc><pay_memo>合成测试</pay_memo></wcpayinfo></appmsg></msg>'),
                  row(5, 1, 200, 4, "wxid_member:\n末端点")]
    shards = [dict(source="message/message_0.db", ids={"1": "wxid_self", "2": "wxid_peer", "3": "synthetic@chatroom", "4": "wxid_member"},
                   tables={"wxid_peer": peer_rows, "synthetic@chatroom": group_rows, "wxid_empty": []}),
              dict(source="message/message_1.db", ids={"1": "wxid_peer", "2": "wxid_self"},
                   tables={"wxid_peer": [row(9, 1, 100, 1, "不同库相同 local_id"), row(13, 1, 190, 2, b"")]}),
              dict(source="message/message_2.db", ids={}, tables={})]
    def raw_value(raw):
        return None if raw["kind"] == "null" else bytes(raw["value"]) if raw["kind"] == "bytes" else raw["value"]
    cases, results = [], []
    with tempfile.TemporaryDirectory(prefix="wx-delta-query-oracle-") as root:
        root = Path(root)
        paths = []
        for index, shard in enumerate(shards):
            path = root / Path(shard["source"]).name
            paths.append(path)
            with closing(sqlite3.connect(path)) as connection, connection:
                connection.execute("CREATE TABLE Name2Id(user_name TEXT)")
                connection.executemany("INSERT INTO Name2Id(rowid,user_name) VALUES(?,?)", shard["ids"].items())
                for username, rows in shard["tables"].items():
                    table = "Msg_" + hashlib.md5(username.encode()).hexdigest()
                    connection.execute(f"CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content,WCDB_CT_message_content INTEGER)")
                    connection.executemany(f"INSERT INTO [{table}] VALUES(?,?,?,?,?,?)", [
                        (m["local_id"], m["local_type"], m["timestamp"], m["sender_id"], raw_value(m["raw"]), m["compression"]) for m in rows])
        for username in ["wxid_peer", "synthetic@chatroom", "wxid_empty", "wxid_missing"]:
            group = username.endswith("@chatroom")
            table = "Msg_" + hashlib.md5(username.encode()).hexdigest()
            ctx = dict(username=username, display_name=names[username], is_group=group, message_tables=[
                dict(db_path=str(path), table_name=table) for shard, path in zip(shards, paths) if username in shard["tables"]])
            server._resolve_chat_context = lambda u: ctx
            result = scope["export_delta_one"](username, str(root / "output"), names, window["run_id"], window["start"], window["end"])
            results.append(result)
            document = None
            if result.get("path"):
                document = json.loads((root / "output/deltas" / window["run_id"] / result["path"]).read_text(encoding="utf-8"))
            model = None
            if result["success"]:
                messages = []
                for shard, path in zip(shards, paths):
                    if username not in shard["tables"]: continue
                    with closing(sqlite3.connect(path)) as connection:
                        ids = scope["_load_name2id_maps"](connection)
                        rows = scope["_query_messages"](connection, table, start_ts=100, end_ts=200, limit=None, oldest_first=True)
                        for record in rows:
                            lid, kind, timestamp, _, raw, ct = record
                            rendered, extras = scope["_extract_content"](lid, kind, raw, ct, username, names[username])
                            messages.append(dict(db_path=shard["source"], local_id=lid, timestamp=timestamp,
                                sender=scope["_resolve_sender"](record, ctx, names, ids), msg_type=scope["_msg_type_str"](kind),
                                raw_content=row(lid, kind, timestamp, 0, raw)["raw"], rendered=rendered,
                                extras={**(extras or {}), "source": shard["source"]}))
                messages.sort(key=lambda m: m["timestamp"])
                contact = scope["_contact_metadata_for_export"](username, group) if not group else dict(contact_remark="", contact_nick_name="", contact_tags=[], contact_memo="")
                model = dict(username=username, display_name=names[username], is_group=group, contact=contact, messages=messages, source_error=None)
            cases.append(dict(username=username, model=model, result=result, document=document))
        manifest = scope["_write_delta_manifest"](str(root / "output"), window["run_id"], 100, 200, len(cases), results)
        manifest = json.loads(manifest.read_text(encoding="utf-8"))
    return dict(window=window, names=names, contacts=contacts, shards=shards, cases=cases, manifest=manifest)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--test-rust", action="store_true")
    args = parser.parse_args()
    try:
        import zstandard
    except ImportError:
        with tempfile.TemporaryDirectory(prefix="wx-delta-query-python-") as dependency_dir:
            # 仅临时测试环境安装；不触及仓库依赖或用户 Python 环境。
            execute([sys.executable, "-m", "pip", "install", "--disable-pip-version-check", "--target", dependency_dir, "zstandard==0.23.0"], "delta-query-oracle-deps.log")
            # 子进程退出后释放 Windows pyd 句柄，再删除临时测试依赖。
            env = dict(os.environ, PYTHONPATH=dependency_dir + os.pathsep + os.environ.get("PYTHONPATH", ""))
            execute([sys.executable, "-X", "utf8", str(Path(__file__).resolve()), *sys.argv[1:]], "delta-query-oracle.log", env=env)
        return
    actual = generate(zstandard)
    fixture = HERE / "golden.json"
    if args.check:
        assert json.loads(fixture.read_text(encoding="utf-8")) == actual
        print("PASS: real synthetic SQLite + legacy AST oracle matches golden", flush=True)
    else:
        fixture.write_text(json.dumps(actual, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"WROTE: {fixture}", flush=True)
    if args.test_rust:
        runpy.run_path(str(HERE / "run_harness.py"), run_name="__main__")


if __name__ == "__main__":
    main()
