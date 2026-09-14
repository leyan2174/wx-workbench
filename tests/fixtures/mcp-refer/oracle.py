"""纯合成 AST 对照：不导入旧 MCP 服务，也不读取账号配置。"""
import argparse
import ast
from contextlib import closing
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]


def generate():
    vendor = REPO / "vendor/wechat-decrypt/mcp_server.py"
    source = vendor.read_text(encoding="utf-8")
    functions = {"_collapse_text", "_parse_xml_root", "_parse_int", "_parse_app_message_outer",
                 "_split_msg_type", "_parse_message_content", "_extract_refer_info",
                 "_summarize_refer_content", "_resolve_quote_sender_label", "_display_name_for_username", "decode_refer"}
    constants = {"_REFER_INNER_TYPE_LABEL", "_INNER_APPMSG_TYPE_LABEL"}
    tree = ast.parse(source)
    selected = []
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name in functions:
            node.decorator_list = []
            selected.append(node)
        elif isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id in constants for t in node.targets):
            selected.append(node)
    tree.body = selected
    names = {"wxid_peer": "合成联系人", "wxid_self": "合成自己", "wxid_member": "合成成员", "room@chatroom": "合成群"}
    ns = dict(re=re, ET=ET, os=os, sqlite3=sqlite3, closing=closing, datetime=datetime,
              _XML_PARSE_MAX_LEN=20_000, _RECORD_XML_PARSE_MAX_LEN=500_000,
              _XML_UNSAFE_RE=re.compile(r"<!\s*(?:DOCTYPE|ENTITY)", re.I),
              _get_self_username=lambda: "wxid_self", get_contact_names=lambda: names,
              resolve_username=lambda chat: "room@chatroom" if chat == "合成群" else "wxid_peer",
              _is_safe_msg_table_name=lambda table: bool(re.fullmatch(r"Msg_[0-9a-f]{32}", table)),
              _decompress_content=lambda content, ct: content)
    exec(compile(tree, str(vendor), "exec"), ns)
    inputs = [("text", "1", " 原文\n  多行 ", "wxid_peer", "原显示名", "合成联系人")]
    for kind in ["3", "34", "42", "43", "47", "48", "50", "999", ""]:
        inputs.append((f"media-{kind}", kind, '<img aeskey="SECRET" cdn="SECRET"/>', "wxid_member", "显示名", "合成群"))
    for kind in ["5", "6", "8", "19", "33", "36", "51", "57", "2000", "2001", "99", ""]:
        inner = f"<msg><appmsg><type>{kind}</type><title> 内层\n标题 </title><url>SECRET</url></appmsg></msg>"
        inputs.append((f"card-{kind}", "49", inner, "wxid_self", "合成自己", "合成联系人"))
    inputs.extend([
        ("empty", "1", "", "", "", "合成联系人"),
        ("unicode-limit", "1", "测" * 161, "", "合成自己", "合成联系人"),
        ("group-fallback", "3", "x", "unknown-user", "ignored-display", "合成群"),
        ("group-self", "1", "自己的消息", "wxid_self", "合成自己", "合成群"),
        ("group-display", "1", "没有账号", "", "群显示名", "合成群"),
        ("display-fallback", "1", "ok", "unknown-user", "fallback-display", "合成联系人"),
        ("broken-inner", "49", "<broken", "", "", "合成联系人"),
        ("unsafe-inner", "49", '<!DOCTYPE msg [<!ENTITY x "SECRET">]><msg><appmsg><title>&x;</title></appmsg></msg>', "", "", "合成联系人"),
    ])
    cases = []
    with tempfile.TemporaryDirectory(prefix="wx-refer-oracle-") as directory:
        path = Path(directory) / "message_0.db"
        with closing(sqlite3.connect(path)) as conn:
            for index, (name, kind, content, sender, display, chat) in enumerate(inputs):
                username = ns["resolve_username"](chat)
                table = "Msg_" + hashlib.md5(username.encode()).hexdigest()
                conn.execute(f"CREATE TABLE IF NOT EXISTS [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER)")
                body = f"<msg><appmsg><title> 回复\n正文 </title><type>57</type><refermsg><type>{kind}</type><fromusr>{sender}</fromusr><chatusr>wxid_member</chatusr><displayname>{display}</displayname><content>{escape(content)}</content><svrid>18446744073709551615</svrid><createtime>0</createtime></refermsg></appmsg></msg>"
                xml = ("wxid_member:" if username.endswith("@chatroom") else "") + body
                conn.execute(f"INSERT INTO [{table}] VALUES(?,49,100,?,0)", (index + 1, xml))
                conn.commit()
                ns["_find_msg_tables_for_user"] = lambda user, table=table: [{"db_path": str(path), "table_name": table}]
                info = ns["_extract_refer_info"](ET.fromstring(body).find("appmsg"))
                expected = {key: value for key, value in info.items() if key != "refer_content"}
                expected.update(refer_type_label=ns["_REFER_INNER_TYPE_LABEL"].get(kind, ""),
                                refer_summary=ns["_summarize_refer_content"](kind, content),
                                refer_sender=ns["_resolve_quote_sender_label"](sender, display, username.endswith("@chatroom"), username, chat, names))
                cases.append(dict(name=name, username=username, chat=chat, xml=xml,
                                  expected=expected, text=ns["decode_refer"](chat, index + 1, 0)))
    return dict(vendor_sha256=hashlib.sha256(source.encode()).hexdigest(), names=names, cases=cases)


def test_rust():
    # 只修改临时副本的模块注册；普通复制源文件，绝不使用硬链接。
    with tempfile.TemporaryDirectory(prefix="wx-refer-build-") as directory:
        root = Path(directory).resolve()
        for folder in ["src", "tests"]:
            shutil.copytree(REPO / folder, root / folder)
        media = Path("vendor/wechat-decrypt/sns_media_wasm")
        shutil.copytree(REPO / media, root / media)
        for filename in ["Cargo.toml", "Cargo.lock", "build.rs"]:
            shutil.copy2(REPO / filename, root / filename)
        query = root / "src/daemon/query.rs"
        # 直接编译副本中的共享 XML 和摘要接口；主线程接线后不重复注册。
        if not re.search(r"\bmod\s+mcp_refer\s*;", query.read_text(encoding="utf-8")):
            with query.open("a", encoding="utf-8") as file:
                file.write('\n#[path = "query/mcp_refer.rs"]\nmod mcp_refer;\n')
        env = os.environ.copy()
        logs = REPO / "target" / "test-logs"
        logs.mkdir(parents=True, exist_ok=True)
        for mode in ["check", "test"]:
            command = ["cargo", mode, "--manifest-path", str(root / "Cargo.toml"), "--target", "x86_64-pc-windows-msvc",
                       "--target-dir", str(REPO / "target"), "--offline", "--bin", "wx"]
            if mode == "test":
                command.extend(["daemon::query::mcp_refer::tests", "--", "--nocapture"])
            print("COMMAND:", subprocess.list2cmdline(command), flush=True)
            result = subprocess.run(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                    text=True, encoding="utf-8", errors="replace")
            log = logs / f"mcp-refer-{mode}.log"
            log.write_text(result.stdout, encoding="utf-8")
            print(result.stdout, flush=True)
            print(f"EXIT: {result.returncode}; FULL LOG: {log}", flush=True)
            if result.returncode:
                raise SystemExit(result.returncode)


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--test-rust", action="store_true")
    args = parser.parse_args()
    data = generate()
    path = HERE / "golden.json"
    if args.write:
        path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"WROTE: {path}; {len(data['cases'])} synthetic vendor cases", flush=True)
    else:
        assert json.loads(path.read_text(encoding="utf-8")) == data
        print(f"PASS: {len(data['cases'])} synthetic vendor cases match", flush=True)
    if args.test_rust:
        test_rust()
