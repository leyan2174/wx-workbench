"""仅 AST 提取纯函数；不导入旧脚本，不接触账号、环境或网络。"""
import ast
import html
import json
import re
from pathlib import Path
from urllib.parse import quote

root = Path(__file__).resolve().parents[3]
source = root / "vendor/wechat-decrypt/export_sns_album.py"
names = {"sns_image_url_candidates", "fix_sns_image_url"}
tree = ast.parse(source.read_text(encoding="utf-8"))
selected = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in names]
assert len(selected) == len(names)
scope = {"html": html, "re": re, "quote": quote}
exec(compile(ast.Module(body=selected, type_ignores=[]), str(source), "exec"), scope)
urls = ["", "  ", " HTTP://example.invalid/a/150 ", "hTtP://example.invalid/a/200?x=1&amp;y=2",
        "https://example.invalid/480", "https://example.invalid/0", "https://example.invalid/150#f",
        "https://example.invalid/150?TOKEN=", "https://example.invalid/150?x=/200?z=/480",
        "https://example.invalid/a?notoken=x", "https://example.invalid/150?x=1&ToKeN=old",
        "https://example.invalid/150/", "&Tab;http://example.invalid/200&#x1f;", "ftp://example.invalid/480",
        "https://example.invalid/150?x=&notit;&amp;&unknown;&#0;&#128;&#xD800;&#xFFFF;",
        "https://example.invalid/150?x=&#999999999999999999999;", "https://example.invalid/150?x=&CounterClockwiseContourIntegral;"]
tokens = ["", " a/b +&=?#%~._- ", "中文令牌", "\x1c token \x1f"]
cases = [{"url": u, "token": t, "candidates": scope["sns_image_url_candidates"](u, t),
          "fixed": scope["fix_sns_image_url"](u, t)} for u in urls for t in tokens]
print(json.dumps(cases, ensure_ascii=True))
