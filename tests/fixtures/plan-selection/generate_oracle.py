"""旧计划消费者 AST oracle；不导入旧服务或读取用户数据。"""
import ast
import csv
import hashlib
import json
import os
from pathlib import Path
import tempfile

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[2] / "vendor/wechat-decrypt/export_all_chats.py"
raw = SOURCE.read_bytes()
tree = ast.parse(raw.decode("utf-8-sig"))
functions = {"_validate_plan_mode", "_write_plan_csv", "_load_selected_usernames_from_plan_csv"}
constants = {"PLAN_MODE_BLACKLIST", "PLAN_MODE_WHITELIST", "PLAN_CSV_FIELDS"}
nodes = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in functions
         or isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id in constants for t in node.targets)]
scope = dict(csv=csv, os=os)
exec(compile(ast.Module(body=nodes, type_ignores=[]), str(SOURCE), "exec"), scope)
assert len(scope["PLAN_CSV_FIELDS"]) == 12
cases = []
rows = [
    [dict(username="peer", export="", chat_name="not peer"), dict(username="other", export="0")],
    [dict(username="other", export="1", chat_name="same"), dict(username="peer", export="1", chat_name="same")],
    [dict(username="peer", export="yes"), dict(username="other", export=" 1 ")],
    [dict(username=" peer ", export=" 0 "), dict(username="other", export="")],
    [dict(username="missing", export="0"), dict(username="peer", export="1")],
    [dict(username="missing", export="1")],
    [dict(username="peer", export="0"), dict(username="peer", export="1")],
    [dict(username="", export="0")],
    [dict(username="peer", chat_name='带逗号,及"引号"\n换行', export="1")],
    [],
]
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / "plan.csv"
    texts = []
    for sample in rows:
        scope["_write_plan_csv"](str(path), sample)
        texts.append(path.read_bytes().decode("utf-8"))
    texts.extend(["username\r\npeer\r\nother\r\n", "export,chat_name\r\n1,peer\r\n", "username,export\r\n", " username,export\r\npeer,1\r\n"])
    for text in texts:
        for mode in ["blacklist", "whitelist"]:
            for valid in [["peer", "other"], ["peer"]]:
                path.write_bytes(text.encode("utf-8"))
                try:
                    expected = scope["_load_selected_usernames_from_plan_csv"](str(path), set(valid), mode)
                    error = False
                except ValueError:
                    expected, error = [], True
                cases.append(dict(csv=text, mode=mode, valid=valid, expected=expected, error=error))
result = dict(source_sha256=hashlib.sha256(raw).hexdigest(), fields=scope["PLAN_CSV_FIELDS"], cases=cases)
output = HERE / "oracle.json"
output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(f"legacy plan CSV AST: {len(cases)} cases, 12 columns; source SHA256 {result['source_sha256']}; {output}")
