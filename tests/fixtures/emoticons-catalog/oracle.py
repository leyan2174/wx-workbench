"""只提取旧函数的映射 AST，输入为测试生成的明文 SQLite。"""
import ast
import json
import re
import sqlite3
import sys
from pathlib import Path

source = Path(sys.argv[1]).read_text(encoding="utf-8")
tree = ast.parse(source)
original = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "build_emoji_lookup")
mapping_try = next(n for n in original.body if isinstance(n, ast.Try) and any(
    isinstance(s, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "lookup" for t in s.targets)
    for s in n.body
))
start = next(i for i, n in enumerate(mapping_try.body) if isinstance(n, ast.Assign)
             and any(isinstance(t, ast.Name) and t.id == "lookup" for t in n.targets))
stop = next(i for i, n in enumerate(mapping_try.body) if isinstance(n, ast.Expr)
            and isinstance(n.value, ast.Call) and isinstance(n.value.func, ast.Attribute)
            and n.value.func.attr == "close")
body = mapping_try.body[start:stop]
body += ast.parse("result = dict(items=[dict(md5=k, info=v) for k,v in lookup.items()], non_store_count=non_store_count, store_added=store_added)").body
module = ast.fix_missing_locations(ast.Module(body=body, type_ignores=[]))
with sqlite3.connect(Path(sys.argv[2]).resolve().as_uri() + "?mode=ro", uri=True) as conn:
    scope = {"conn": conn, "re": re}
    exec(compile(module, "legacy-mapping-ast", "exec"), scope)
    print(json.dumps(scope["result"], ensure_ascii=True))
