"""Synthetic-only AST oracle: execute actual legacy functions without importing apps."""
import argparse
import ast
from contextlib import closing
import json
import pathlib
import sqlite3
import tempfile
import types
import unicodedata

ROOT = pathlib.Path(__file__).resolve().parents[1]


def extract(path, names, namespace):
    tree = ast.parse(path.read_text(encoding="utf-8-sig"))
    selected = [n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in names]
    assert {n.name for n in selected} == set(names)
    exec(compile(ast.Module(body=selected, type_ignores=[]), str(path), "exec"), namespace)


def fixtures():
    base = "CREATE TABLE contact(username, nick_name, remark, description, local_type, extra_buffer);"
    labels = "CREATE TABLE contact_label(label_id_, label_name_, sort_order_);"
    cases = [
        ("full", base + labels + "INSERT INTO contact VALUES ('u','Nick','Remark','Memo',0,X'F20105322C312C32'); INSERT INTO contact_label VALUES (1,'First',0),(2,'Second',1);", "u", False),
        ("nulls", base + "INSERT INTO contact VALUES ('u',NULL,NULL,NULL,0,NULL);", "u", False),
        ("empty", base + "INSERT INTO contact VALUES ('u','','','',0,NULL);", "u", False),
        ("old_schema", "CREATE TABLE contact(username,nick_name,remark); INSERT INTO contact VALUES ('u','Old','');", "u", False),
        ("missing_required", "CREATE TABLE contact(username,remark); INSERT INTO contact VALUES ('u','R');", "u", False),
        ("missing_table", "CREATE TABLE unrelated(id);", "u", False),
        ("missing_user", base, "absent", False),
        ("group", base, "u", True),
        ("excluded_group_member", base + labels + "INSERT INTO contact VALUES ('u','N','R','M',3,X'F2010131'); INSERT INTO contact_label VALUES (1,'Tag',0);", "u", False),
        ("excluded_null_type", base + "INSERT INTO contact VALUES ('u','N','R','M',NULL,NULL);", "u", False),
        ("duplicate_contact", base + "INSERT INTO contact VALUES ('u','first',NULL,NULL,0,NULL),('u','second','R','M',0,NULL);", "u", False),
        ("duplicate_label", base + labels + "INSERT INTO contact VALUES ('u','N','R','M',0,X'F20105312C312C32'); INSERT INTO contact_label VALUES (1,'Old',0),(2,'Other',1),(1,'New',2);", "u", False),
        ("empty_label", base + labels + "INSERT INTO contact VALUES ('u','','','',0,X'F20105312C322C33'); INSERT INTO contact_label VALUES (1,NULL,0),(2,'',1),(3,' ',2);", "u", False),
        ("missing_extra_buffer", "CREATE TABLE contact(username,nick_name,remark); INSERT INTO contact VALUES ('u','N','R');" + labels + "INSERT INTO contact_label VALUES (1,'T',0);", "u", False),
        ("text_label_id", base + labels + "INSERT INTO contact VALUES ('u','','','',0,X'F2010131'); INSERT INTO contact_label VALUES ('1','Not integer',0);", "u", False),
        ("wrong_buffer_type", base + labels + "INSERT INTO contact VALUES ('u','','','',0,'bad'); INSERT INTO contact_label VALUES (1,'T',0);", "u", False),
        ("other_bad_buffer", base + labels + "INSERT INTO contact VALUES ('u','','','',0,X'F2010131'),('other','','','',0,'bad'); INSERT INTO contact_label VALUES (1,'T',0);", "u", False),
        ("ignored_extra_fields", "CREATE TABLE contact(username,nick_name,remark,alias,province,city,sex,signature); INSERT INTO contact VALUES ('u','N','R','Alias','P','C',1,'S');", "u", False),
        ("numeric_metadata", base + "INSERT INTO contact VALUES ('u',42,0,2.5,0,NULL);", "u", False),
        ("empty_username", base + labels + "INSERT INTO contact VALUES ('','N','R','M',0,X'F2010131'); INSERT INTO contact_label VALUES (1,'T',0);", "", False),
        ("uppercase_required", "CREATE TABLE contact(USERNAME,NICK_NAME,REMARK,DESCRIPTION,LOCAL_TYPE); INSERT INTO contact VALUES ('u','N','R','ignored',3);", "u", False),
        ("nocase_username", "CREATE TABLE contact(username TEXT COLLATE NOCASE,nick_name,remark); INSERT INTO contact VALUES ('U','Wrong','');", "u", False),
        ("quote_username", base + "INSERT INTO contact VALUES ('x'' OR 1=1--','N','R','M',0,NULL);", "x' OR 1=1--", False),
        ("unicode_text", base + "INSERT INTO contact VALUES ('u','\u6635\u79f0','\u5907\u6ce8','\u7b7e\u540d\n\u591a\u884c',0,NULL);", "u", False),
        ("float_label_id", base + labels + "INSERT INTO contact VALUES ('u','','','',0,X'F2010131'); INSERT INTO contact_label VALUES (1.0,'T',0);", "u", False),
        ("missing_label_sort", base + "CREATE TABLE contact_label(label_id_,label_name_); INSERT INTO contact_label VALUES (1,'T');", "u", False),
        ("missing_required_with_tags", "CREATE TABLE contact(username,extra_buffer); INSERT INTO contact VALUES ('u',X'F2010131');" + labels + "INSERT INTO contact_label VALUES (1,'T',0);", "u", False),
        ("underscore_label_id", base + labels + "INSERT INTO contact VALUES ('u','','','',0,X'F20109305F312C312C5F315F'); INSERT INTO contact_label VALUES (1,'T',0);", "u", False),
    ]
    buffers = ["F2010131", "F2010931", "F20101FF", "F201", "08AC02F2010131", "090000000000000000F2010131", "0D00000000F2010131", "03F2010131", "F2010131F2010132", "F20108202B312C782C3220", "F20104312C2C31", "80"]
    for i, buf in enumerate(buffers):
        cases.append((f"protobuf_{i}", base + labels + f"INSERT INTO contact VALUES ('u','','','',0,X'{buf}'); INSERT INTO contact_label VALUES (1,'One',0),(2,'Two',1);", "u", False))
    for codepoint in range(0x110000):
        char = chr(codepoint)
        if unicodedata.category(char) == "Nd" and unicodedata.decimal(char) == 1:
            encoded = char.encode("utf-8")
            buf = (b"\xf2\x01" + bytes([len(encoded)]) + encoded).hex()
            cases.append((f"unicode_digit_{codepoint:x}", base + labels + f"INSERT INTO contact VALUES ('u','','','',0,X'{buf}'); INSERT INTO contact_label VALUES (1,'One',0);", "u", False))
    return cases


def generate():
    result = []
    with tempfile.TemporaryDirectory(prefix="wx-synthetic-contact-") as temp:
        path = pathlib.Path(temp) / "contact.db"
        for name, sql, username, is_group in fixtures():
            if path.exists():
                path.unlink()
            with closing(sqlite3.connect(path)) as conn:
                conn.executescript(sql)
            ns = {"sqlite3": sqlite3, "_contact_names": None, "_contact_full": None,
                  "_contact_tags": None, "_get_contact_db_path": lambda: str(path)}
            extract(ROOT / "vendor/wechat-decrypt/mcp_server.py",
                    ["_load_contacts_from", "get_contact_names", "get_contact_full",
                     "get_contact_tag_names_by_username", "_extract_pb_field_30", "_load_contact_tags"], ns)
            export = {"mcp_server": types.SimpleNamespace(**ns)}
            extract(ROOT / "vendor/wechat-decrypt/export_all_chats.py", ["_contact_metadata_for_export"], export)
            expected = export["_contact_metadata_for_export"](username, is_group)
            result.append(dict(name=name, sql=sql, username=username, is_group=is_group, expected=expected))
    return json.dumps(result, ensure_ascii=True, indent=2) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    output = ROOT / "tests/fixtures/contact-metadata-golden.json"
    generated = generate()
    if args.check:
        assert output.read_text(encoding="utf-8") == generated, "golden differs from legacy AST oracle"
    else:
        output.write_text(generated, encoding="utf-8")
    print(f"{len(fixtures())} synthetic AST oracle cases: {'verified' if args.check else 'generated'}")
