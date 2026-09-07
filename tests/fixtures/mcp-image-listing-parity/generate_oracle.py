"""从旧源码 AST 运行真实 SQLite/文件元数据链；不导入旧服务或访问真实账号。"""
import argparse
import ast
import glob
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from contextlib import closing
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
DECODE = ROOT / "vendor/wechat-decrypt/decode_image.py"
MCP = ROOT / "vendor/wechat-decrypt/mcp_server.py"
USER = "wxid_fixture"
OTHER = "wxid_other"
BASE = 1704067200
MD5_A = hashlib.md5(b"synthetic resource A").hexdigest()
MD5_B = hashlib.md5(b"synthetic resource B").hexdigest()


class FixtureDateTime(datetime):
    """固定旧本机时间依赖为 Asia/Shanghai 对应的 UTC+8，不改函数体。"""
    @classmethod
    def fromtimestamp(cls, timestamp, tz=None):
        return super().fromtimestamp(timestamp, tz or timezone(timedelta(hours=8)))

    def timestamp(self):
        value = self if self.tzinfo else self.replace(tzinfo=timezone(timedelta(hours=8)))
        return datetime.timestamp(value)


def ast_scope():
    decoder_raw = DECODE.read_bytes()
    mcp_raw = MCP.read_bytes()
    decoder_tree = ast.parse(decoder_raw.decode("utf-8-sig"))
    mcp_tree = ast.parse(mcp_raw.decode("utf-8-sig"))
    methods = {"__init__", "get_image_md5", "find_dat_files", "list_chat_images"}
    nodes = []
    locations = {}
    for node in decoder_tree.body:
        if isinstance(node, ast.FunctionDef) and node.name == "extract_md5_from_packed_info":
            nodes.append(node)
            locations[node.name] = node.lineno
        elif isinstance(node, ast.ClassDef) and node.name == "ImageResolver":
            node.body = [method for method in node.body
                         if isinstance(method, ast.FunctionDef) and method.name in methods]
            for method in node.body:
                locations[f"ImageResolver.{method.name}"] = method.lineno
            nodes.append(node)
    scope = dict(os=os, glob=glob, hashlib=hashlib, sqlite3=sqlite3, sys=sys)
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(DECODE), "exec"), scope)
    functions = {"_validate_pagination", "_parse_time_value", "_parse_time_range",
                 "_pagination_hint", "get_chat_images"}
    nodes = []
    for node in mcp_tree.body:
        if isinstance(node, ast.FunctionDef) and node.name in functions:
            node.decorator_list = []
            nodes.append(node)
            locations[node.name] = node.lineno
        elif isinstance(node, ast.Assign) and any(
                isinstance(target, ast.Name) and target.id == "_QUERY_LIMIT_MAX"
                for target in node.targets):
            nodes.append(node)
    scope["datetime"] = FixtureDateTime
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(MCP), "exec"), scope)
    return scope, {
        "decode_image.py": hashlib.sha256(decoder_raw).hexdigest(),
        "mcp_server.py": hashlib.sha256(mcp_raw).hexdigest(),
    }, locations


def message(local_id=1, timestamp=BASE, kind=3, shard=0):
    return dict(local_id=local_id, create_time=timestamp, local_type=kind, shard=shard)


def resource(local_id=1, timestamp=BASE, md5=MD5_A, username=USER, kind=3):
    blob = b"\x12\x22\x0a\x20" + md5.encode("ascii") if md5 else b"not protobuf MD5"
    return dict(username=username, local_id=local_id, create_time=timestamp,
                local_type=kind, packed_hex=blob.hex())


def dat(md5=MD5_A, size=1536, month="2024-01", suffix="", username=USER):
    return dict(username=username, relative=f"{month}/Img/{md5}{suffix}.dat", size=size)


def cases():
    baseline = dict(messages=[message()], resources=[resource()], files=[dat()])
    def case(name, **changes):
        return dict(name=name, **(baseline | changes))
    return [
        case("unique_resource_one_dat"),
        case("missing_resource_row", resources=[]),
        case("missing_resource_database", resource_available=False),
        case("resource_without_md5", resources=[resource(md5="")]),
        case("md5_but_not_downloaded", files=[]),
        case("zero_byte_dat", files=[dat(size=0)]),
        case("half_kib_rounds_to_even", files=[dat(size=512)]),
        case("reused_local_id_legacy_chooses_newest", resources=[resource(), resource(timestamp=BASE+60, md5=MD5_B)], files=[dat(), dat(md5=MD5_B, size=8192)]),
        case("missing_exact_time_both_fallback", resources=[resource(timestamp=BASE+60, md5=MD5_B)], files=[dat(md5=MD5_B, size=8192)]),
        case("other_chat_same_id", resources=[resource(username=OTHER, md5=MD5_B), resource()], files=[dat(), dat(username=OTHER, md5=MD5_B, size=8192)]),
        case("high_flag_resource", resources=[resource(kind=(1 << 32) + 3)]),
        case("high_flag_message_legacy_excluded", messages=[message(kind=(1 << 32) + 3)], resources=[resource(kind=(1 << 32) + 3)]),
        case("ambiguous_exact_resource_legacy_first_row", resources=[resource(), resource(md5=MD5_B)], files=[dat(), dat(md5=MD5_B, size=8192)]),
        case("legacy_cross_month_lexical_thumbnail", messages=[message(timestamp=1707955200)], resources=[resource(timestamp=1707955200)], files=[dat(size=3072, month="2024-01", suffix="_t"), dat(size=8192, month="2024-02")]),
        case("legacy_prefix_glob_accepts_noncanonical_name", files=[dat(suffix="UNRELATED", size=777)]),
        case("uppercase_marker_preserved", resources=[resource(md5=MD5_A.upper())], files=[dat(md5=MD5_A.upper())]),
        case("global_shard_page", messages=[message(1, BASE, shard=0), message(2, BASE+120, shard=1), message(3, BASE+60, shard=0)], resources=[resource(1), resource(2, BASE+120, MD5_B), resource(3, BASE+60)], files=[dat(), dat(md5=MD5_B, size=8192)], kwargs=dict(limit=1, offset=1)),
        case("time_bounds_inclusive", messages=[message(1, BASE-1), message(2, BASE), message(3, BASE+60), message(4, BASE+61)], resources=[], files=[], kwargs=dict(start_time="2024-01-01 08:00:00", end_time="2024-01-01 08:01:00")),
        case("offset_exhausted", kwargs=dict(offset=10)),
        case("invalid_pagination", kwargs=dict(limit=0)),
        case("invalid_time", kwargs=dict(start_time="not-a-time")),
        case("no_shards", messages=[]),
        case("unknown_chat", chat_name="unknown"),
    ]


def snapshot(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in root.rglob("*") if path.is_file()}


def evaluate(scope, spec):
    with tempfile.TemporaryDirectory(prefix="wx-image-listing-oracle-") as directory:
        root = Path(directory)
        db = root / "message_resource.db"
        with closing(sqlite3.connect(db)) as conn, conn:
            conn.execute("CREATE TABLE ChatName2Id(user_name TEXT)")
            conn.executemany("INSERT INTO ChatName2Id(rowid,user_name) VALUES(?,?)", [(1, USER), (2, OTHER)])
            conn.execute("CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB)")
            for row in spec["resources"]:
                conn.execute("INSERT INTO MessageResourceInfo VALUES(?,?,?,?,?)", (1 if row["username"] == USER else 2, row["local_id"], row["local_type"], row["create_time"], bytes.fromhex(row["packed_hex"])))
        shards = []
        for shard in sorted({row["shard"] for row in spec["messages"]}):
            path = root / f"message_{shard}.db"
            with closing(sqlite3.connect(path)) as conn, conn:
                conn.execute("CREATE TABLE Msg_demo(local_id INTEGER,local_type INTEGER,create_time INTEGER)")
                conn.executemany("INSERT INTO Msg_demo VALUES(?,?,?)", [(row["local_id"], row["local_type"], row["create_time"]) for row in spec["messages"] if row["shard"] == shard])
            shards.append(dict(db_path=str(path), table_name="Msg_demo"))
        for file in spec["files"]:
            path = root / "msg/attach" / hashlib.md5(file["username"].encode()).hexdigest() / file["relative"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"x" * file["size"])
        accesses = []
        def cache_get(key):
            assert key == "message/message_resource.db"
            accesses.append(key)
            return str(db) if spec.get("resource_available", True) else None
        resolver = scope["ImageResolver"](str(root), str(root / "MUST_NOT_CREATE"), SimpleNamespace(get=cache_get))
        scope.update(_image_resolver=resolver,
                     resolve_username=lambda name: USER if name == "Demo" else None,
                     get_contact_names=lambda: {USER: "Demo", OTHER: "Demo"},
                     _find_msg_tables_for_user=lambda username: shards if username == USER else [])
        before = snapshot(root)
        ACTIVE[0] = True
        try:
            kwargs = spec.get("kwargs", {})
            text = scope["get_chat_images"](spec.get("chat_name", "Demo"), **kwargs)
            rows = []
            for shard in shards:
                rows.extend(resolver.list_chat_images(shard["db_path"], shard["table_name"], USER, limit=100))
        finally:
            ACTIVE[0] = False
        assert snapshot(root) == before, "legacy listing changed synthetic input files"
        assert not (root / "MUST_NOT_CREATE").exists()
        for row in rows:
            if "dat_file" in row:
                row["dat_file"] = Path(row["dat_file"]).relative_to(root).as_posix()
        return dict(**spec, legacy_rows=rows, legacy_text=text,
                    resource_cache_reads=len(accesses), source_unchanged=True,
                    dat_content_reads=0, output_created=False)


ACTIVE = [False]
def audit(event, args):
    if ACTIVE[0] and event == "open" and isinstance(args[0], (str, bytes, os.PathLike)):
        assert not os.fsdecode(args[0]).lower().endswith(".dat"), "listing attempted DAT content read"
sys.addaudithook(audit)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    scope, hashes, locations = ast_scope()
    result = dict(source_sha256=hashes, source_lines=locations, timezone="UTC+08:00",
                  note="Synthetic resource MD5 is stored metadata, not a claim about DAT plaintext; size is encrypted DAT bytes selected by legacy lexical glob.",
                  cases=[evaluate(scope, spec) for spec in cases()])
    target = HERE / "oracle.json"
    if args.check:
        assert json.loads(target.read_text(encoding="utf-8")) == result, "oracle drift"
    else:
        target.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"PASS legacy AST oracle: {len(result['cases'])} cases; read-only snapshots; zero DAT content reads")
    for source, digest in hashes.items():
        print(f"SOURCE {source} SHA256={digest}")
    print(target)


if __name__ == "__main__":
    main()
