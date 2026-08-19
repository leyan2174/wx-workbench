"""Export one contact's locally cached Moments text and images as an HTML album."""

from __future__ import annotations

import argparse
import html
import json
import re
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from urllib.request import Request, urlopen


WECHAT_EMOJI_MAP = {
    "微笑": "🙂", "笑脸": "😄", "愉快": "😄", "嘿哈": "😆", "撇嘴": "😒",
    "色": "😍", "发呆": "😳", "得意": "😎", "流泪": "😢", "害羞": "😊",
    "闭嘴": "🤐", "睡": "😴", "大哭": "😭", "尴尬": "😅", "发怒": "😡",
    "调皮": "😜", "呲牙": "😁", "惊讶": "😮", "难过": "😔", "酷": "😎",
    "冷汗": "😓", "抓狂": "😫", "偷笑": "🤭", "白眼": "🙄", "悠闲": "😌",
    "奋斗": "💪", "疑问": "❓", "嘘": "🤫", "晕": "😵", "囧": "😳",
    "再见": "👋", "擦汗": "😅", "抠鼻": "🙄", "鼓掌": "👏", "坏笑": "😏",
    "哈欠": "🥱", "鄙视": "😒", "委屈": "🥺", "亲亲": "😘", "可怜": "🥺",
    "西瓜": "🍉", "啤酒": "🍺", "咖啡": "☕", "饭": "🍚", "玫瑰": "🌹",
    "凋谢": "🥀", "嘴唇": "💋", "爱心": "❤️", "心碎": "💔", "蛋糕": "🎂",
    "闪电": "⚡", "炸弹": "💣", "月亮": "🌙", "太阳": "☀️", "礼物": "🎁",
    "拥抱": "🤗", "强": "👍", "弱": "👎", "握手": "🤝", "胜利": "✌️",
    "耶": "✌️", "抱拳": "🙏", "合十": "🙏", "拳头": "✊", "爱你": "🤟",
    "NO": "🙅", "OK": "👌", "庆祝": "🎉", "捂脸": "🤦", "烟花": "🎆",
    "爆竹": "🧨",
}


def clean_text(value: object) -> str:
    text = str(value or "").replace("\r\n", "\n").replace("\r", "\n")
    text = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f]", "", text)
    return re.sub(r"\n{3,}", "\n\n", text).strip()


def render_text(value: object) -> str:
    escaped = html.escape(clean_text(value))

    def replace(match: re.Match[str]) -> str:
        emoji = WECHAT_EMOJI_MAP.get(match.group(1))
        return f'<span class="emoji">{emoji}</span>' if emoji else match.group(0)

    return re.sub(r"\[([^\[\]\s]{1,8})\]", replace, escaped).replace("\n", "<br>")


def safe_stem(value: object, fallback: str) -> str:
    cleaned = "".join("_" if ch in '<>:"/\\|?*' else ch for ch in str(value or "")).strip()
    return cleaned.rstrip(". ") or fallback


def run_wx_sns_feed(
    wx_exe: Path,
    user: str,
    limit: int,
    since: str | None,
    until: str | None,
) -> list[dict]:
    command = [str(wx_exe), "sns-feed", "--user", user, "-n", str(limit), "--json"]
    if since:
        command.extend(["--since", since])
    if until:
        command.extend(["--until", until])

    last_error = ""
    for attempt in range(1, 7):
        try:
            proc = subprocess.run(
                command,
                check=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=30,
            )
        except subprocess.TimeoutExpired:
            last_error = "wx sns-feed timed out after 30 seconds"
            time.sleep(min(attempt * 2, 10))
            continue
        if proc.returncode == 0 and proc.stdout.strip():
            cleaned = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f]", "", proc.stdout)
            data = json.loads(cleaned)
            if not isinstance(data, list):
                raise RuntimeError("wx sns-feed did not return a JSON list")
            return data
        last_error = clean_text(proc.stderr or proc.stdout)
        time.sleep(min(attempt * 2, 10))
    raise RuntimeError(f"wx sns-feed failed after retries: {last_error}")


def detect_image_extension(data: bytes) -> str | None:
    if data.startswith(b"\xff\xd8\xff"):
        return "jpg"
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    if data.startswith((b"GIF87a", b"GIF89a")):
        return "gif"
    if len(data) >= 12 and data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "webp"
    return None


def save_image(data: bytes, destination: Path, media: dict) -> bool:
    extension = detect_image_extension(data)
    if not extension:
        return False
    destination = destination.with_suffix(f".{extension}")
    destination.write_bytes(data)
    media["local_file"] = f"images/{destination.name}"
    return True


def download_plain_image(url: str, destination: Path, media: dict) -> bool:
    headers = {
        "User-Agent": "MicroMessenger Client",
        "Accept": "*/*",
        "Referer": "https://mp.weixin.qq.com/",
    }
    try:
        with urlopen(Request(url, headers=headers), timeout=5) as response:
            data = response.read(25 * 1024 * 1024 + 1)
        if not data or len(data) > 25 * 1024 * 1024:
            return False
        return save_image(data, destination, media)
    except Exception:
        return False


def load_cache_helpers():
    import export_sns  # type: ignore

    cache_index = export_sns._build_sns_cache_index()
    return export_sns, cache_index, [entry[0] for entry in cache_index]


def write_cache_image(export_sns, matched_path: str, destination: Path, media: dict) -> bool:
    data = export_sns._decrypt_sns_dat(matched_path)
    return bool(data) and save_image(data, destination, media)


def post_images(post: dict) -> list[dict]:
    return [
        media
        for media in post.get("media", [])
        if str(media.get("type")) == "2" and media.get("local_file")
    ]


def readable_posts(posts: list[dict]) -> list[dict]:
    return [post for post in posts if clean_text(post.get("content")) or post_images(post)]


def image_tag(media: dict) -> str:
    src = html.escape(str(media["local_file"]), quote=True)
    dimensions = ""
    try:
        width = int(media.get("width") or 0)
        height = int(media.get("height") or 0)
        if width > 0 and height > 0:
            dimensions = f' width="{width}" height="{height}"'
    except (TypeError, ValueError):
        pass
    return f'<a href="{src}" target="_blank"><img src="{src}"{dimensions} loading="lazy" alt=""></a>'


def build_html(user: str, posts: list[dict], output: Path) -> int:
    visible = readable_posts(posts)
    years: list[str] = []
    for post in visible:
        year = str(post.get("time") or "")[:4]
        if year.isdigit() and year not in years:
            years.append(year)

    parts = [
        "<!doctype html>",
        '<html lang="zh-CN"><head><meta charset="utf-8">',
        '<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">',
        f"<title>{html.escape(user)}朋友圈</title>",
        "<style>",
        "*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;background:#fafafa;color:#202124;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI','Microsoft YaHei',sans-serif;letter-spacing:0;line-height:1.7}",
        "header{max-width:780px;margin:0 auto;padding:42px 18px 20px}h1{margin:0;font-size:30px;font-weight:700}.year-nav{position:sticky;top:0;z-index:3;display:flex;gap:8px;overflow-x:auto;padding:10px max(14px,calc((100vw - 780px)/2 + 18px));background:rgba(250,250,250,.94);border-block:1px solid #e5e7eb;backdrop-filter:blur(12px)}",
        ".year-nav a{flex:0 0 auto;color:#334155;text-decoration:none;padding:5px 10px;border-radius:6px;background:#eef2f7;font-size:14px}main{max-width:780px;margin:0 auto;padding:0 18px 52px}.year{scroll-margin-top:62px;margin:34px 0 8px;font-size:24px}.post{padding:20px 0;border-top:1px solid #e5e7eb}.time{color:#64748b;font-size:14px;margin-bottom:7px}.content{white-space:normal;font-size:16px;overflow-wrap:anywhere}.emoji{font-family:'Segoe UI Emoji','Apple Color Emoji',sans-serif}.imgs{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:7px;margin-top:13px}.imgs a{display:block;min-width:0}.imgs img{display:block;width:100%;height:auto;max-height:620px;object-fit:cover;background:#e5e7eb;border-radius:6px}",
        "@media(max-width:560px){header{padding:28px 14px 14px}h1{font-size:25px}main{padding:0 14px 40px}.year{font-size:21px}.post{padding:17px 0}.imgs{grid-template-columns:repeat(2,minmax(0,1fr))}.imgs:has(a:only-child){grid-template-columns:1fr}.imgs:has(a:only-child) img{object-fit:contain}}",
        "</style></head><body>",
        f"<header><h1>{html.escape(user)}朋友圈</h1></header>",
    ]
    if years:
        parts.append('<nav class="year-nav" aria-label="年份">')
        parts.extend(f'<a href="#year-{year}">{year}</a>' for year in years)
        parts.append("</nav>")
    parts.append("<main>")

    current_year = None
    for post in visible:
        year = str(post.get("time") or "")[:4]
        if year.isdigit() and year != current_year:
            parts.append(f'<h2 class="year" id="year-{year}">{year}</h2>')
            current_year = year
        parts.append('<article class="post">')
        parts.append(f'<div class="time">{html.escape(str(post.get("time") or ""))}</div>')
        content = clean_text(post.get("content"))
        if content:
            parts.append(f'<div class="content">{render_text(content)}</div>')
        images = post_images(post)
        if images:
            parts.append('<div class="imgs">')
            parts.extend(image_tag(media) for media in images)
            parts.append("</div>")
        parts.append("</article>")
    parts.extend(["</main></body></html>"])
    output.write_text("\n".join(parts), encoding="utf-8", newline="\n")
    return len(visible)


def export_album(args: argparse.Namespace) -> dict:
    posts = run_wx_sns_feed(args.wx_exe, args.user, args.limit, args.since, args.until)
    run_id = datetime.now().strftime("%Y%m%d-%H%M%S")
    output_dir = args.output_root / f"{safe_stem(args.user, 'contact')}-朋友圈相册-{run_id}"
    image_dir = output_dir / "images"
    image_dir.mkdir(parents=True, exist_ok=False)

    export_sns, cache_index, index_mtimes = load_cache_helpers()
    image_cache = 0
    image_remote = 0
    image_missing = 0
    videos = 0
    for post_index, post in enumerate(posts, 1):
        image_items = []
        for media_index, media in enumerate(post.get("media", []), 1):
            media_type = str(media.get("type"))
            if media_type == "6":
                videos += 1
                continue
            if media_type != "2":
                continue
            tid = safe_stem(post.get("tid"), str(post_index))
            image_items.append((media, image_dir / f"{post_index:05d}_{tid}_{media_index:02d}.jpg"))

        timestamp = int(post.get("timestamp") or 0)
        matches = export_sns._match_cache_images(
            timestamp,
            [media for media, _ in image_items],
            cache_index,
            index_mtimes,
        ) if timestamp and image_items else [(None, None)] * len(image_items)

        for (media, destination), (matched_path, _format) in zip(image_items, matches):
            if matched_path and write_cache_image(export_sns, matched_path, destination, media):
                image_cache += 1
                continue
            downloaded = False
            if not args.no_remote:
                urls = []
                for key in ("url", "thumb"):
                    value = str(media.get(key) or "").strip()
                    if value and value not in urls:
                        urls.append(value)
                downloaded = any(download_plain_image(url, destination, media) for url in urls)
            if downloaded:
                image_remote += 1
            else:
                image_missing += 1

    timeline_path = output_dir / "timeline.json"
    html_path = output_dir / "timeline.html"
    timeline_path.write_text(json.dumps(posts, ensure_ascii=False, indent=2), encoding="utf-8")
    album_posts = build_html(args.user, posts, html_path)
    summary = {
        "user": args.user,
        "posts": len(posts),
        "album_posts": album_posts,
        "first": posts[-1].get("time") if posts else None,
        "last": posts[0].get("time") if posts else None,
        "image_ok": image_cache + image_remote,
        "image_cache": image_cache,
        "image_remote": image_remote,
        "image_missing": image_missing,
        "video_count_not_exported": videos,
        "output_dir": str(output_dir.resolve()),
        "timeline_json": str(timeline_path.resolve()),
        "html": str(html_path.resolve()),
    }
    (output_dir / "export_summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    return summary


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Export one contact's Moments text and images.")
    parser.add_argument("--wx-exe", type=Path, required=True)
    parser.add_argument("--user", required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--limit", type=int, default=5000)
    parser.add_argument("--since")
    parser.add_argument("--until")
    parser.add_argument("--no-remote", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        summary = export_album(args)
    except Exception as error:
        print(f"朋友圈相册导出失败: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
