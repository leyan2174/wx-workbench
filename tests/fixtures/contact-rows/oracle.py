"""只执行旧联系人两个函数的 AST；不导入 MCP、配置或用户数据。"""
import ast
import json
import pathlib
import sqlite3
import sys

source = pathlib.Path(__file__).resolve().parents[3] / "vendor/wechat-decrypt/mcp_server.py"
tree = ast.parse(source.read_text(encoding="utf-8"))
functions = []
for node in tree.body:
    if isinstance(node, ast.FunctionDef) and node.name in {"_load_contacts_from", "get_contacts"}:
        node.decorator_list = []
        functions.append(node)
assert len(functions) == 2
scope = {"sqlite3": sqlite3}
exec(compile(ast.Module(body=functions, type_ignores=[]), str(source), "exec"), scope)
_, full = scope["_load_contacts_from"](sys.argv[1])
scope["get_contact_full"] = lambda: full
query, limit = sys.argv[2], int(sys.argv[3])
selected = [c for c in full if not query or any(query.lower() in c[k].lower() for k in ("username", "nick_name", "remark"))]
print(json.dumps({"contacts": selected[:limit], "total": len(selected), "text": scope["get_contacts"](query, limit)}, ensure_ascii=True))
