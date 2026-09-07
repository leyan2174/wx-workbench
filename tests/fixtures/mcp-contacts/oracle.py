"""Only execute the two legacy label helpers against a supplied synthetic database."""
import ast
import json
from pathlib import Path
import sqlite3
import sys

source = Path(__file__).resolve().parents[3] / "vendor/wechat-decrypt/mcp_server.py"
tree = ast.parse(source.read_text(encoding="utf-8"))
helpers = ast.Module(body=[node for node in tree.body if isinstance(node, ast.FunctionDef)
                          and node.name in {"_extract_pb_field_30", "_load_contact_tags"}], type_ignores=[])
assert len(helpers.body) == 2
scope = {"sqlite3": sqlite3, "_contact_tags": None,
         "_get_contact_db_path": lambda: sys.argv[1],
         "get_contact_names": lambda: {"u1": "张三"}}
exec(compile(helpers, str(source), "exec"), scope)
tags = sorted(scope["_load_contact_tags"]().values(), key=lambda tag: tag["sort_order"])
result = {"total_tags": len(tags), "total_associations": sum(len(tag["members"]) for tag in tags),
          "tags": [{"name": tag["name"], "member_count": len(tag["members"]),
                    "members": tag["members"]} for tag in tags]}
print(json.dumps(result, ensure_ascii=True))
