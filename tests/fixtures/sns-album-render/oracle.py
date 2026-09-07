"""仅从旧脚本 AST 提取白名单函数，不导入或执行旧脚本入口。"""
import ast
import html
import json
from pathlib import Path
import re
import struct
import zlib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
names = {"clean_text", "render_text", "safe_stem", "post_images", "post_videos", "readable_posts", "image_tag", "video_tag", "build_html"}
tree = ast.parse((ROOT / "vendor/wechat-decrypt/export_sns_album.py").read_text(encoding="utf-8"))
nodes = [n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in names or isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "WECHAT_EMOJI_MAP" for t in n.targets)]
assert {n.name for n in nodes if isinstance(n, ast.FunctionDef)} == names
scope = {"html": html, "re": re, "Path": Path, "VIDEO_TYPES": {"6", "15"}}
exec(compile(ast.Module(body=nodes, type_ignores=[]), "<album-pure-oracle>", "exec"), scope)

class Sink:
    def write_text(self, text, **kwargs):
        self.html = text

values = [None, False, True, 0, 2, 2.0, "  A\r\n\rB\u0000\n\n\nC  ", "[微笑][未知表情][爱心]<script>&\"'", [], [True, None, "hello"], {}, {"a": False}, "文件<>:\"/\\|?*. ", "\u001c x \u001c"]
cases = []
values.extend([1e-5, 1e20, 1e15, -0.0, 1234567890123456789012345678901234567890])
timelines = [[], [{}], [{"content": v, "time": "2024-03-01"} for v in values], [
    {"time": "2023-01-02", "media": [{"type": 2, "local_file": "images/a.png", "width": 123, "height": "45"}]},
    {"time": "2025-02-03", "media": [{"type": "6", "local_file": "videos/a.mp4"}]},
    {"time": "2023-03-04", "content": "重复年份[强]", "media": [{"type": 15, "local_file": "videos/b.mp4"}]},
    {"time": "<img onerror=x>", "content": "<script>alert('x')</script>"},
    {"media": [{"type": 2.0, "local_file": "images/ignored.png"}, {"type": True, "local_file": "images/ignored.png"}]},
], [{"time": t, "content": "year"} for t in ["２０２４", "²³", "Ⅳ", "①", "202", None, False]]]
for posts in timelines:
    sink = Sink()
    count = scope["build_html"]("合成<&\"'", posts, sink)
    cases.append({"posts": posts, "html": sink.html, "count": count})
for width in ["１_２３", "+0012", "1234567890123456789012345678901234567890", "1__2", "-2", 1e25, 2.9, True, {}, None]:
    posts = [{"media": [{"type": 2, "local_file": "images/a.png", "width": width, "height": 12}]}]
    sink = Sink()
    count = scope["build_html"]("合成<&\"'", posts, sink)
    cases.append({"posts": posts, "html": sink.html, "count": count})
print(json.dumps({"values": [{"value": v, "clean": scope["clean_text"](v), "text": scope["render_text"](v), "stem": scope["safe_stem"](v, "fallback")} for v in values], "emoji": [{"name": k, "text": scope["render_text"](f"[{k}]")} for k in scope["WECHAT_EMOJI_MAP"]], "cases": cases}, ensure_ascii=True))

if __name__ == "__main__" and "--assets" in __import__("sys").argv:
    # 标准库生成真实 RGB PNG，无真实照片、远端资源或个人数据。
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    width, height = 480, 320
    pixels = b"".join(b"\0" + bytes(c for x in range(width) for c in ((42, 130, 110) if x < 240 and y < 160 else (235, 193, 62) if y < 160 else (80, 135, 205) if x < 240 else (222, 92, 111))) for y in range(height))
    image_dir = HERE / "images"
    image_dir.mkdir(exist_ok=True)
    (image_dir / "synthetic.png").write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b""))
    video_dir = HERE / "videos"
    video_dir.mkdir(exist_ok=True)
    (video_dir / "placeholder.mp4").write_bytes(b"")
