"""Execute real legacy export AST against public synthetic SQLite and file trees.

No vendor imports, config execution, private cache access, or network imports.
Run with python -B <this-file>; only temporary files are written at runtime.
"""
import ast
import base64
import binascii
from contextlib import closing, redirect_stdout
from datetime import datetime, timedelta, timezone
import hashlib
import html
from html.parser import HTMLParser
import io
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import tempfile
from types import SimpleNamespace
import unittest
import xml.etree.ElementTree as ET


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SOURCE = ROOT / "vendor/wechat-decrypt/export_sns.py"
ZONE = timezone(timedelta(hours=8))
INPUT = json.loads((HERE / "input.json").read_text(encoding="utf-8"))
GOLDEN = json.loads((HERE / "golden.json").read_text(encoding="utf-8"))


class Clock:
    fromtimestamp = staticmethod(lambda value: datetime.fromtimestamp(value, ZONE))
    now = staticmethod(lambda: datetime.fromtimestamp(1700001000, ZONE))


def extract():
    tree = ast.parse(SOURCE.read_text(encoding="utf-8-sig"))
    functions = {
        "_decode_sns_content_blob", "_sanitize_sns_pseudo_xml", "_parse_media_list",
        "_parse_timeline_xml", "_load_comments", "_safe_dirname", "_timestamp_filename",
        "_load_contact_map", "_html_escape", "_generate_timeline_html",
        "_detect_format", "export_sns_timeline",
    }
    nodes = [node for node in tree.body if
             (isinstance(node, ast.FunctionDef) and node.name in functions) or
             (isinstance(node, ast.Assign) and all(
                 isinstance(t, ast.Name) and
                 (t.id.startswith("_SNS_") or t.id == "_CONTENT_TYPES")
                 for t in node.targets))]
    assert {n.name for n in nodes if isinstance(n, ast.FunctionDef)} == functions
    namespace = dict(base64=base64, binascii=binascii, html=html, re=re, ET=ET,
                     datetime=Clock, sqlite3=sqlite3, json=json)
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(SOURCE), "exec"), namespace)
    return namespace


def xml(row):
    root = ET.Element("TimelineObjects")
    post = ET.SubElement(root, "TimelineObject")
    for name, value in {"id": "synthetic-" + str(row["tid"]),
                        "username": row["xml_author"], "createTime": row["time"],
                        "contentDesc": row["text"]}.items():
        ET.SubElement(post, name).text = str(value)
    content = ET.SubElement(post, "ContentObject")
    ET.SubElement(content, "type").text = "1" if row.get("media") else "2"
    if row.get("media"):
        media = ET.SubElement(ET.SubElement(post, "mediaList"), "media")
        ET.SubElement(media, "type").text = "2"
        ET.SubElement(media, "url").text = "https://example.invalid/synthetic.png"
    extra = ET.SubElement(root, "LocalExtraInfo")
    ET.SubElement(extra, "nickname").text = "Synthetic XML Nick"
    return ET.tostring(root, encoding="unicode")


class Page(HTMLParser):
    def __init__(self, path):
        super().__init__()
        self.images, self.texts = [], []
        self.in_text = False
        self.feed(path.read_text(encoding="utf-8"))

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "img":
            self.images.append(attrs["src"])
        if tag == "div" and attrs.get("class") == "post-text":
            self.in_text = True

    def handle_data(self, data):
        if self.in_text:
            self.texts.append(data)

    def handle_endtag(self, tag):
        if tag == "div":
            self.in_text = False


def snapshot(path):
    return {p.relative_to(path).as_posix(): (p.read_bytes(), p.stat().st_mtime_ns)
            for p in sorted(path.rglob("*")) if p.is_file()}


class LegacyOracle(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="synthetic-sns-oracle-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.output = self.root / "output"
        self.db = self.root / "sns.db"
        self.contacts = self.root / "contact.db"
        with closing(sqlite3.connect(self.db)) as db, db:
            db.executescript("CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content TEXT);"
                             "CREATE TABLE SnsMessage_tmp3(feed_id, create_time, type, from_username,"
                             "from_nickname, to_username, to_nickname, content, del_status);")
        with closing(sqlite3.connect(self.contacts)) as db, db:
            db.execute("CREATE TABLE contact(username, remark, nick_name)")
            db.executemany("INSERT INTO contact VALUES(?,?,?)", INPUT["contacts"])
        self.cache = self.root / "synthetic-cache.png"
        self.cache.write_bytes(base64.b64decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII="))
        self.old = extract()
        self.calls = []
        # Supply explicit temporary paths instead of executing load_config (which
        # can auto-discover real accounts and write config.json).
        self.old.update(SNS_DB_PATH=str(self.db), CONTACT_DB_PATH=str(self.contacts),
                        OUTPUT_DIR=str(self.output), _CONTACT_FILTER=None,
                        os=SimpleNamespace(path=os.path, makedirs=os.makedirs,
                                           environ={"WECHAT_SNS_DOWNLOAD_MEDIA": "1"}),
                        _build_sns_cache_index=lambda: [(1700000000, str(self.cache))],
                        _match_cache_images=lambda t, media, index, times:
                        [(str(self.cache), "png")] * len(media),
                        _decrypt_sns_dat=self.decrypt,
                        _try_download_media=self.download)

    def decrypt(self, path):
        self.assertEqual(Path(path), self.cache)
        return self.cache.read_bytes()

    def download(self, url, path):
        self.assertEqual(url, "https://example.invalid/synthetic.png")
        self.assertTrue(Path(path).is_relative_to(self.output))
        self.calls.append(url)
        return False

    def rows(self, rows):
        with closing(sqlite3.connect(self.db)) as db, db:
            db.execute("DELETE FROM SnsTimeLine")
            db.executemany("INSERT INTO SnsTimeLine VALUES(?,?,?)",
                           [(r["tid"], r["db_author"], xml(r)) for r in rows])

    def run_export(self):
        capture = io.StringIO()
        with redirect_stdout(capture):
            self.old["export_sns_timeline"]()
        print(capture.getvalue().replace(str(self.root), "<TEMP>"), end="")

    def read(self, directory, name="timeline.json"):
        return json.loads((directory / name).read_text(encoding="utf-8"))

    def test_two_runs_and_empty_rows(self):
        self.rows(INPUT["first"])
        self.run_export()
        sns = self.output / "Synthetic Author/SNS"
        filtered = self.output / "Filtered Author/SNS"
        first = self.read(sns)
        self.assertEqual([p["tid"] for p in first["posts"]], GOLDEN["first_tids"])
        self.assertEqual(len(Page(sns / "timeline.html").images), 2)
        (sns / "notes").mkdir()
        (sns / "notes/keep.txt").write_bytes(b"synthetic nested unrelated")
        (sns / "unrelated.bin").write_bytes(b"synthetic unrelated")
        # Distinct old mtimes allow unchanged-byte rewrites to be detected too.
        for file in self.output.rglob("*"):
            if file.is_file():
                os.utime(file, ns=(1000000000, 1000000000))
        before, filtered_before = snapshot(sns), snapshot(filtered)
        self.rows(INPUT["second"])
        self.old["_CONTACT_FILTER"] = {"db_author"}
        self.old["_build_sns_cache_index"] = lambda: []
        self.run_export()
        after = snapshot(sns)
        self.assertEqual(self.read(sns), GOLDEN["second_summary"])
        self.assertEqual(self.read(sns, "20231115061320000.json"),
                         GOLDEN["second_summary"]["posts"][0])
        self.assertEqual(sorted(after), GOLDEN["second_files"])
        rewritten = {"20231115061320000.json", "timeline.json", "timeline.html"}
        for name in before:
            if name in rewritten:
                self.assertNotEqual(before[name][0], after[name][0], name)
                self.assertNotEqual(before[name][1], after[name][1], name)
            else:
                self.assertEqual(before[name], after[name], name)
        self.assertEqual(snapshot(filtered), filtered_before)
        page = Page(sns / "timeline.html")
        self.assertEqual(page.texts, GOLDEN["second_html_text"])
        self.assertEqual(page.images, GOLDEN["second_html_images"])
        self.assertEqual(self.calls, ["https://example.invalid/synthetic.png"])
        self.assertNotIn("local_file", self.read(sns)["posts"][0]["media"][0])
        all_before = snapshot(self.output)
        self.rows([])
        self.old["_build_sns_cache_index"] = lambda: self.fail("empty rows scanned cache")
        self.run_export()
        self.assertEqual(snapshot(self.output), all_before)
        self.assertEqual(len(self.calls), 1)

    def test_same_second_names_are_run_local_not_post_identity(self):
        rows = [dict(tid=n, db_author="db_author", xml_author="xml_other", time=0,
                     text="COLLISION " + str(n)) for n in (10, 11)]
        sns = self.output / "Synthetic Author/SNS"
        for phase, batch in (("first", rows), ("second", rows[1:])):
            self.rows(batch)
            self.run_export()
            actual = {p.name: self.read(sns, p.name)["tid"]
                      for p in sns.glob("0*.json")}
            self.assertEqual(actual, GOLDEN["collision_" + phase])
        self.assertEqual([p["tid"] for p in self.read(sns)["posts"]], [11])

    def test_null_and_empty_database_authors_do_not_use_xml_author(self):
        self.rows([dict(tid=i, db_author=db, xml_author=author, time=0, text="AUTHOR")
                   for i, db, author in ((20, None, "xml_a"), (21, "", "xml_b"))])
        self.run_export()
        summary = self.read(self.output / "Synthetic XML Nick/SNS")
        self.assertEqual(summary["user_name"], GOLDEN["null_author_summary"])
        self.assertEqual([p["db_user_name"] for p in summary["posts"]],
                         GOLDEN["null_author_post_db_names"])
        self.assertEqual([p["username"] for p in summary["posts"]],
                         GOLDEN["null_author_post_xml_names"])


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    print("Legacy source SHA256:", hashlib.sha256(SOURCE.read_bytes()).hexdigest())
    unittest.main(verbosity=2)
