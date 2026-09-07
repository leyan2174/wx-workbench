"""只用 AST 截取旧工具的纯解析部分；不 import mcp_server、不打开真实数据库或缓存。"""
import ast
import copy
import hashlib
import json
import os
from pathlib import Path
import re
from types import SimpleNamespace
import xml.etree.ElementTree as ET
from contextlib import closing

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[2] / "vendor/wechat-decrypt/mcp_server.py"
HELPERS = {
    "_split_msg_type", "_decompress_content", "_parse_message_content", "_collapse_text",
    "_safe_basename", "_parse_xml_root", "_parse_int", "_parse_app_message_outer",
    "_is_safe_msg_table_name",
}
CONSTANTS = {"_XML_UNSAFE_RE", "_XML_PARSE_MAX_LEN", "_RECORD_XML_PARSE_MAX_LEN", "_RECORD_DATATYPE_LABEL"}


def projection(mode):
    # 只投影旧工具已经算出的值，不另写一套 XML 解析器。
    if mode == "file":
        return """return dict(kind='file', item_index=None, item_count=None, datatype=None,
title=title, extension=fileext, expected_size=totallen or None,
expected_md5=expected_md5 or None, sender='', description='')"""
    return """return dict(kind={'1':'text','2':'image','4':'voice','5':'video','8':'file'}.get(datatype,'metadata_only'),
item_index=item_index, item_count=len(items), datatype=datatype,
title=datatitle, extension=datafmt, expected_size=datasize or None,
expected_md5=expected_md5 or None, sender=sourcename,
description=_collapse_text(item.findtext('datadesc') or ''))"""


def oracle():
    tree = ast.parse(SOURCE.read_text(encoding="utf-8"))
    nodes = []
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id in CONSTANTS for t in node.targets):
            nodes.append(node)
        if isinstance(node, ast.FunctionDef) and node.name in HELPERS:
            nodes.append(node)
        if isinstance(node, ast.FunctionDef) and node.name in {"decode_file_message", "decode_record_item"}:
            fn = copy.deepcopy(node)
            fn.decorator_list = []
            mode = "file" if fn.name == "decode_file_message" else "record"
            end = next(i for i, statement in enumerate(fn.body) if
                (mode == "file" and isinstance(statement, ast.Assign)
                 and any(isinstance(t, ast.Name) and t.id == "candidates" for t in statement.targets))
                or (mode == "record" and isinstance(statement, ast.If)
                    and ast.unparse(statement.test) == "datatype == '1'"))
            fn.body = fn.body[:end] + ast.parse(projection(mode)).body
            nodes.append(fn)
    # 合成 SQL 只返回当前 case 的行；工具函数被截断在任何附件 I/O 之前。
    class Connection:
        def execute(self, *_args):
            return self
        def fetchone(self):
            return current["row"]
        def close(self):
            pass
    current = {}
    ns = dict(re=re, ET=ET, os=os, closing=closing,
              sqlite3=SimpleNamespace(connect=lambda _path: Connection()),
              resolve_username=lambda chat: chat,
              _find_msg_tables_for_user=lambda username: [dict(db_path="synthetic.db", table_name="Msg_" + hashlib.md5(username.encode()).hexdigest())])
    exec(compile(ast.fix_missing_locations(ast.Module(body=nodes, type_ignores=[])), str(SOURCE), "exec"), ns)
    return ns, current


def outer(kind, content):
    return f"<msg><appmsg><type>{kind}</type>{content}</appmsg></msg>"


def record(items):
    return outer(19, "<recorditem><![CDATA[<recordinfo><datalist>" + "".join(items) + "</datalist></recordinfo>]]></recorditem>")


def item(kind, title="", extra=""):
    return f'<dataitem datatype="{kind}"><datatitle>{title}</datatitle>{extra}</dataitem>'


def cases():
    out = []
    def add(name, mode, body, index=0, username="wxid_synthetic"):
        out.append(dict(name=name, mode=mode, username=username, source="message/message_12.db", local_id=7, create_time=123, body=body, index=index))
    file_body = outer(6, "<title>报告.pdf</title><appattach><fileext>pdf</fileext><totallen>3</totallen></appattach><md5>900150983cd24fb0d6963f7d28e17f72</md5>")
    add("file-full", "file", file_body)
    add("file-group-inline", "file", "sender:<" + file_body[1:], username="room@chatroom")
    add("file-group-newline", "file", "sender:\n" + file_body, username="room@chatroom")
    add("file-no-hash-size", "file", outer(6, "<title>report[1].txt</title><appattach/>"))
    add("file-collapse", "file", outer(6, "<title>two  spaces.txt</title><appattach><totallen>+1_024</totallen></appattach>"))
    add("file-upper-hash", "file", file_body.replace("900150983cd24fb0d6963f7d28e17f72", "900150983CD24FB0D6963F7D28E17F72"))
    for kind in ["1", "2", "3", "4", "5", "6", "7", "8", "17", "19", "22", "23", "29", "36", "37", "999"]:
        add("record-" + kind, "record", record([item(kind, "sample", "<sourcename> sender  name </sourcename><datadesc> hello  world </datadesc><datasize>3</datasize><datafmt>txt</datafmt>")]))
    add("record-file-hash", "record", record([item("8", "report.txt", "<datasize>3</datasize><fullmd5>900150983CD24FB0D6963F7D28E17F72</fullmd5>")]))
    add("record-size-only", "record", record([item("4", "", "<datasize>3</datasize>")]))
    add("record-index-60", "record", record([item("1", "", f"<datadesc>item {i}</datadesc>") for i in range(61)]), 60)
    add("record-group", "record", "sender:" + record([item("2")]), username="room@chatroom")
    add("record-over-20k", "record", record([item("1", "", "<datadesc>" + "测" * 22_000 + "</datadesc>")]))
    add("record-no-nested-expansion", "record", record([item("17", "nested", "<recorditem><dataitem datatype='8'><datatitle>hidden.txt</datatitle></dataitem></recorditem>"), item("1", "", "<datadesc>second</datadesc>")]), 1)
    return out


def main():
    ns, current = oracle()
    results = cases()
    for case in results:
        current["row"] = (49, case["create_time"], case["body"], 0)
        fn = ns["decode_file_message" if case["mode"] == "file" else "decode_record_item"]
        args = [case["username"], case["local_id"]]
        if case["mode"] == "record":
            args.append(case["index"])
        case["expected"] = fn(*args, create_time=case["create_time"])
        if not isinstance(case["expected"], dict):
            raise AssertionError((case["name"], case["expected"]))
    payload = dict(source="vendor/wechat-decrypt/mcp_server.py", source_sha256=hashlib.sha256(SOURCE.read_bytes()).hexdigest(), cases=results)
    (HERE / "golden.json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"AST golden: {len(results)} cases; no module import, SQL, cache scan, or network")


if __name__ == "__main__":
    main()
