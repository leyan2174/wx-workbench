"""从旧 SNS 纯函数提取合成 golden；不导入旧模块，不读取配置、账号或 keys。"""
import ast
import base64
import binascii
import hashlib
import html
import html.entities
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import tempfile
from datetime import datetime, timezone, timedelta
import xml.etree.ElementTree as ET

import zstandard as zstd

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "tests" / "fixtures" / "sns"
ZONE = timezone(timedelta(hours=8))


class Clock:
    @staticmethod
    def fromtimestamp(value):
        return datetime.fromtimestamp(value, ZONE)

    @staticmethod
    def now():
        return datetime.fromtimestamp(1700001000, ZONE)


def legacy_namespace():
    source = ROOT / "vendor" / "wechat-decrypt" / "export_sns.py"
    tree = ast.parse(source.read_text(encoding="utf-8-sig"))
    allowed = {
        "_decode_sns_content_blob", "_sanitize_sns_pseudo_xml", "_parse_media_list",
        "_parse_timeline_xml", "_load_comments", "_safe_dirname", "_timestamp_filename",
        "_load_contact_map", "_html_escape", "_generate_timeline_html", "export_sns_timeline",
    }
    nodes = []
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name in allowed:
            nodes.append(node)
        elif isinstance(node, ast.Assign) and all(
            isinstance(t, ast.Name) and (t.id.startswith("_SNS_") or t.id == "_CONTENT_TYPES")
            for t in node.targets
        ):
            nodes.append(node)
    assert {n.name for n in nodes if isinstance(n, ast.FunctionDef)} == allowed
    namespace = dict(base64=base64, binascii=binascii, html=html, re=re, ET=ET,
                     zstd=zstd, datetime=Clock, sqlite3=sqlite3, os=os, json=json)
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), "exec"), namespace)
    return namespace, hashlib.sha256(source.read_bytes()).hexdigest()


def xml(body="", kind="2", stamp="1700000000", ident="18446744073709551610", nickname="合成昵称"):
    return (f"<TimelineObjects><TimelineObject><id>{ident}</id>"
            f"<username>wxid_synthetic</username><createTime>{stamp}</createTime>"
            f"<ContentObject><type>{kind}</type></ContentObject>{body}"
            f"</TimelineObject><LocalExtraInfo><nickname>{nickname}</nickname>"
            "</LocalExtraInfo></TimelineObjects>")


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    old, digest = legacy_namespace()
    rich = xml('<private>1</private><contentDesc>合成 &amp; &lt;3 &#x1f642; &copy; &notin;</contentDesc>'
               '<location latitude="31.2" longitude="121.4" poiName="合成地点"/>'
               '<mediaList><media><type>2</type><sub_type>7</sub_type>'
               '<videoDuration>1.5</videoDuration>'
               '<url md5="synthetic-md5" key="synthetic-key" token="synthetic-token">'
               'https://example.invalid/photo?a=1&amp;b=2</url><thumb key="t" token="tt"/>'
               '<size width="800" height="600" totalSize="12345"/></media>'
               '<media><type>6</type><url>https://example.invalid/video</url></media>'
               '<media/></mediaList>', kind="1")
    cases = []

    def add(name, payload, encoding="text"):
        expected = old["_parse_timeline_xml"](payload)
        cases.append(dict(name=name, encoding=encoding,
                          input=base64.b64encode(payload).decode() if encoding == "blob_base64" else payload,
                          expected=expected))

    add("rich", rich)
    add("raw_utf8", rich.encode(), "blob_base64")
    add("zstd", zstd.ZstdCompressor().compress(rich.encode()), "blob_base64")
    add("hex", rich.encode().hex())
    add("base64", base64.b64encode(rich.encode()).decode())
    add("hex_zstd", zstd.ZstdCompressor().compress(rich.encode()).hex())
    add("base64_zstd", base64.b64encode(zstd.ZstdCompressor().compress(rich.encode())).decode())
    add("whitespace_hex", " \n".join(rich.encode().hex()[i:i+60] for i in range(0, len(rich.encode().hex()), 60)))
    add("invalid_utf8", rich.encode().replace(b"<private>", b"\xff<private>"), "blob_base64")
    add("pseudo_xml", xml('<contentDesc>hello\x01 <3 > 2 & bare</contentDesc>'))
    add("cdata_extra", xml('<extraInfo><![CDATA[a&b<c]]></extraInfo>'))
    add("cdata_text_legacy", xml('<contentDesc><![CDATA[plain]]></contentDesc>'))
    add("entities", xml('<contentDesc>&notit; &euro; &#128; &#0; &#13; &#xFDD0;</contentDesc>'))
    add("default", "<x><TimelineObject/></x>")
    add("empty_fields", xml('<contentDesc/><private/>', kind="", stamp="bad", nickname=""))
    add("negative_timestamp", xml(stamp="-1"))
    add("unknown_type", xml(kind="999"))
    for kind in (2, 3, 5, 7, 15, 28, 30, 34, 42, 54):
        add(f"type_{kind}", xml(kind=str(kind)))
    add("location_zero", xml('<location latitude="0" longitude="0"/>'))
    add("location_decimal_zero", xml('<location latitude="0.0"/>'))
    add("first_descendant", '<x><TimelineObject><id>first</id></TimelineObject><TimelineObject><id>second</id></TimelineObject></x>')
    add("first_matching_child", '<x><TimelineObject><ContentObject/><ContentObject><type>15</type></ContentObject></TimelineObject><LocalExtraInfo/><LocalExtraInfo><nickname>合成后备</nickname></LocalExtraInfo></x>')
    add("bare_root_legacy", '<TimelineObject><id>not-selected</id></TimelineObject>')
    add("html_escaped_xml", html.escape(xml()))
    add("malformed", '<x><TimelineObject></x>')
    add("none", None)
    add("empty", "")
    add("doctype", '<!DOCTYPE x [<!ENTITY test "bad">]>' + xml())
    add("doctype_zstd", zstd.ZstdCompressor().compress(('<!DOCTYPE x>' + xml()).encode()), "blob_base64")

    # SQLite 仅在内存构造，DB 行的输入编码与 Rust 测试共用。
    conn = sqlite3.connect(":memory:")
    conn.executescript("""
        CREATE TABLE SnsMessage_tmp3(feed_id INTEGER, create_time INTEGER, type INTEGER,
            from_username TEXT, from_nickname TEXT, to_username TEXT, to_nickname TEXT,
            content TEXT, del_status INTEGER);
    """)
    comments = [
        [-6, 1700000001, 1, "wxid_like", "合成点赞", None, None, None, 0],
        [-6, 1700000003, 2, "wxid_reply", "合成回复", "wxid_like", "合成点赞", "合成评论", None],
        [-6, 1700000002, 2, "wxid_deleted", "撤回", "", "", "不应出现", 1],
        [7, None, None, None, None, None, None, None, 0],
    ]
    conn.executemany("INSERT INTO SnsMessage_tmp3 VALUES (?,?,?,?,?,?,?,?,?)", comments)
    rows = [dict(tid=-6, user_name="wxid_synthetic", case="rich"),
            dict(tid=7, user_name="wxid_synthetic", case="empty_fields"),
            dict(tid=8, user_name="wxid_synthetic", case="zstd"),
            dict(tid=9, user_name="wxid_fallback", case="negative_timestamp"),
            dict(tid=10, user_name=None, case="default"),
            dict(tid=11, user_name="wxid_synthetic", case="malformed")]
    contacts = [["wxid_synthetic", "合成:备注", "不用昵称"], ["wxid_unused", "", "合成联系人"]]
    by_name = {c["name"]: c for c in cases}
    conn.execute("CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content)")
    for row in rows:
        case = by_name[row["case"]]
        content = base64.b64decode(case["input"]) if case["encoding"] == "blob_base64" else case["input"]
        conn.execute("INSERT INTO SnsTimeLine VALUES (?,?,?)", (row["tid"], row["user_name"], content))
    conn.commit()
    timelines, files = [], {}
    # 直接运行提取的旧导出函数；所有路径均指向本次创建的合成临时目录。
    with tempfile.TemporaryDirectory(prefix="wx-sns-golden-") as directory:
        root = Path(directory)
        sns_path, contact_path, output = root / "sns.db", root / "contact.db", root / "out"
        with sqlite3.connect(sns_path) as target:
            conn.backup(target)
        target.close()
        with sqlite3.connect(contact_path) as target:
            target.execute("CREATE TABLE contact(username TEXT, remark TEXT, nick_name TEXT)")
            target.executemany("INSERT INTO contact VALUES (?,?,?)", contacts)
        target.close()
        old.update(SNS_DB_PATH=str(sns_path), CONTACT_DB_PATH=str(contact_path), OUTPUT_DIR=str(output),
                   _CONTACT_FILTER=None, _build_sns_cache_index=lambda: [])
        os.environ["WECHAT_SNS_DOWNLOAD_MEDIA"] = "0"
        old["export_sns_timeline"]()
        for path in sorted(output.rglob("*.json")):
            data = json.loads(path.read_text(encoding="utf-8"))
            if path.name == "timeline.json":
                timelines.append(data)
            else:
                files[path.relative_to(output).as_posix()] = data
    conn.close()
    timelines.sort(key=lambda item: item["user_name"])
    payload = dict(provenance=dict(source="vendor/wechat-decrypt/export_sns.py", sha256=digest,
                                  synthetic_only=True, timezone="UTC+08:00"),
                   cases=cases, database=dict(rows=rows, contacts=contacts, comments=comments,
                                             timelines=timelines, files=files))
    DEST.mkdir(parents=True, exist_ok=True)
    (DEST / "golden.json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    entities = ROOT / "src" / "toolkit" / "sns" / "html_entities.json"
    entities.write_text(json.dumps(html.entities.html5, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Generated {len(cases)} synthetic parser cases, {len(rows)} database rows, {len(files)} post files")
    print(f"Golden: {DEST / 'golden.json'}")
    print(f"HTML5 entity table: {entities}")
    print(f"Legacy source SHA256: {digest}")


if __name__ == "__main__":
    main()
