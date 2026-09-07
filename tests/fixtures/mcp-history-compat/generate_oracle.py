"""仅抽取旧 AST 纯函数，在合成 SQLite 上生成历史选择 oracle；不导入旧服务。"""
import ast
import hashlib
import json
from pathlib import Path
import re
import sqlite3

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[2] / "vendor/wechat-decrypt/mcp_server.py"
raw = SOURCE.read_bytes()
tree = ast.parse(raw.decode("utf-8-sig"))
functions = {"_resolve_msg_types", "_build_message_filters", "_query_messages", "_page_ranked_entries"}
nodes = [n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in functions
         or isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "_MSG_TYPE_MAP" for t in n.targets)]
scope = {"_is_safe_msg_table_name": lambda s: re.fullmatch(r"Msg_[0-9a-f]{32}", s) is not None}
exec(compile(ast.Module(body=nodes, type_ignores=[]), str(SOURCE), "exec"), scope)
TABLE = "Msg_" + "a" * 32
shards = [
    [[1, 1, 100, 11, "text-a", 0], [2, 3, 110, 12, "image-a", 0],
     [3, 49, 120, 13, "app-a", 0], [4, 34, 120, 14, "voice-a", 0],
     [5, 1, 140, 15, "text-b", 0]],
    [[1, 3, 100, 21, "image-b", 0], [2, 49, 115, 22, "app-b", 0],
     [3, 1, 120, 23, "text-c", 0], [4, 10000, 150, 24, "system", 0]],
    [],
]
cases = []
for names in [None, [], [" Text ", "IMAGE"], ["file", "app", "voice"], ["system"], ["emoji"]]:
    types, err = scope["_resolve_msg_types"](names)
    assert err is None
    for oldest in [False, True]:
        for start, end in [(None, None), (110, 120), (120, 120), (None, 120), (120, None), (-1, 150)]:
            for limit, offset in [(1, 0), (3, 1), (50, 0), (2, 99)]:
                entries = []
                for shard, rows in enumerate(shards):
                    with sqlite3.connect(":memory:") as conn:
                        conn.execute(f"CREATE TABLE {TABLE}(local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content)")
                        conn.executemany(f"INSERT INTO {TABLE} VALUES(?,?,?,?,?,?)", rows)
                        selected = scope["_query_messages"](conn, TABLE, start_ts=start, end_ts=end,
                            limit=limit+offset, oldest_first=oldest, type_filter=types)
                        entries.extend((row[2], [shard, row[0]]) for row in selected)
                expected = [value for _, value in scope["_page_ranked_entries"](entries, limit, offset, oldest)]
                cases.append(dict(names=names, types=types or [], oldest=oldest, since=start, until=end,
                                  limit=limit, offset=offset, expected=expected))
assert scope["_resolve_msg_types"](["unknown"])[1]
result = dict(source_sha256=hashlib.sha256(raw).hexdigest(), table=TABLE, shards=shards, cases=cases)
target = HERE / "oracle.json"
encoded = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
target.write_text(encoded, encoding="utf-8")
print(f"legacy AST + SQLite oracle: {len(cases)} cases; source SHA256 {result['source_sha256']}; output {target}")
