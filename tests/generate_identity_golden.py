"""生成发送者规则的旧实现对照，不导入服务或读取账号数据。"""
import ast
import json
from pathlib import Path

root = Path(__file__).resolve().parents[1]
source = ast.parse((root / "vendor/wechat-decrypt/mcp_server.py").read_text(encoding="utf-8"))
nodes = [node for node in source.body if isinstance(node, ast.FunctionDef) and node.name in {"_display_name_for_username", "_resolve_sender_label"}]
scope = {"_get_self_username": lambda: "self"}
exec(compile(ast.Module(body=nodes, type_ignores=[]), "legacy-identity", "exec"), scope)
names = {"self": "我的昵称", "peer": "联系人"}
cases = []
for group in [False, True]:
    chat = "room@chatroom" if group else "peer"
    for mapped in ["", "self", "peer", "unknown", chat]:
        for prefix in ["", "self", "peer", "unknown"]:
            expected = scope["_resolve_sender_label"](1, prefix, group, chat, "聊天备注", names, {1: mapped})
            cases.append(dict(mapped=mapped, prefix=prefix, group=group, chat=chat, expected=expected))
target = root / "tests/fixtures/identity-golden.json"
target.write_text(json.dumps(cases, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(f"Generated {len(cases)} synthetic legacy sender cases: {target}")
