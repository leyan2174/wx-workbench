"""Export one contact's Moments text, images, and videos as an HTML album."""

from __future__ import annotations

import argparse
import base64
import concurrent.futures
import hashlib
import html
import json
import queue
import re
import shutil
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from urllib.parse import quote
from urllib.request import Request, urlopen


VIDEO_TYPES = {"6", "15"}
VIDEO_DECRYPT_BYTES = 128 * 1024
MAX_VIDEO_BYTES = 2 * 1024 * 1024 * 1024


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


def is_mp4(data: bytes) -> bool:
    return len(data) >= 12 and data[4:8] == b"ftyp"


class WasmKeystream:
    def __init__(self, wasm_dir: Path | None = None):
        self.wasm_dir = wasm_dir or Path(__file__).with_name("sns_media_wasm")
        self.runner = self.wasm_dir / "weflow_wasm_keystream.js"
        self.process: subprocess.Popen[str] | None = None
        self.request_id = 0

    def available(self) -> bool:
        return bool(shutil.which("node")) and self.runner.is_file()

    def generate(self, key: str, size: int) -> bytes:
        if not self.available():
            raise RuntimeError("Node.js or SNS video WASM assets are unavailable")
        if self.process is None:
            self.process = subprocess.Popen(
                ["node", str(self.runner), "--stdio"],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                bufsize=1,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
        self.request_id += 1
        request = {"id": self.request_id, "key": key, "size": size}
        assert self.process.stdin is not None
        assert self.process.stdout is not None
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError("SNS video WASM process exited unexpectedly")
        response = json.loads(line)
        if response.get("error"):
            raise RuntimeError(str(response["error"]))
        data = base64.b64decode(response.get("data") or "")
        if len(data) != size:
            raise RuntimeError(f"SNS video WASM returned {len(data)} bytes, expected {size}")
        return data

    def close(self) -> None:
        if self.process is None:
            return
        if self.process.stdin:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=3)
        self.process = None

    def __enter__(self) -> "WasmKeystream":
        return self

    def __exit__(self, _type, _value, _traceback) -> None:
        self.close()


def video_cache_key(post_id: object, media_id: object) -> str:
    value = f"{str(post_id or '').strip()}_{str(media_id or '').strip()}_3"
    return hashlib.md5(value.encode("utf-8")).hexdigest()


def build_video_cache_index(cache_root: Path | None) -> dict[str, Path]:
    if cache_root is None or not cache_root.is_dir():
        return {}
    index: dict[str, Path] = {}
    for path in cache_root.glob("*/Sns/Video/*/*"):
        if not path.is_file():
            continue
        key = (path.parent.name + path.stem).lower()
        if not re.fullmatch(r"[0-9a-f]{32}", key):
            continue
        previous = index.get(key)
        if previous is None or path.suffix.lower() == ".mp4" or path.stat().st_size > previous.stat().st_size:
            index[key] = path
    return index


def find_cached_video(post: dict, media: dict, index: dict[str, Path]) -> Path | None:
    media_id = str(media.get("id") or "").strip()
    if not media_id:
        return None
    post_ids = [str(post.get("post_id") or "").strip(), str(post.get("tid") or "").strip()]
    try:
        tid = int(post.get("tid"))
        post_ids.append(str(tid & ((1 << 64) - 1)))
    except (TypeError, ValueError):
        pass
    for post_id in dict.fromkeys(value for value in post_ids if value):
        match = index.get(video_cache_key(post_id, media_id))
        if match:
            return match
    return None


def copy_cached_video(
    source: Path,
    destination: Path,
    media: dict,
    allow_partial: bool,
) -> str | None:
    try:
        with source.open("rb") as stream:
            head = stream.read(16)
        if not is_mp4(head):
            return None
        complete = source.suffix.lower() == ".mp4"
        if not complete and not allow_partial:
            return None
        destination = destination.with_suffix(".mp4")
        shutil.copy2(source, destination)
        media["local_file"] = f"videos/{destination.name}"
        media["video_source"] = "cache"
        media["video_complete"] = complete
        return "complete" if complete else "partial"
    except OSError:
        return None


def download_decrypt_video(
    url: str,
    key: str,
    destination: Path,
    media: dict,
    keystream: WasmKeystream,
) -> bool:
    if not url.startswith("http"):
        media["video_error"] = "missing video URL"
        return False
    destination = destination.with_suffix(".mp4")
    temporary = destination.with_suffix(".mp4.part")
    headers = {"User-Agent": "MicroMessenger Client", "Accept": "*/*"}
    try:
        with urlopen(Request(url, headers=headers), timeout=30) as response:
            content_length = int(response.headers.get("Content-Length") or 0)
            if content_length > MAX_VIDEO_BYTES:
                media["video_error"] = "video exceeds the maximum allowed size"
                return False
            prefix = response.read(VIDEO_DECRYPT_BYTES)
            if not prefix:
                media["video_error"] = "video response is empty"
                return False
            if not is_mp4(prefix):
                if not key:
                    media["video_error"] = "encrypted video is missing enc_key"
                    return False
                stream = keystream.generate(key, len(prefix))
                prefix = bytes(value ^ stream[index] for index, value in enumerate(prefix))
            if not is_mp4(prefix):
                media["video_error"] = "decrypted video header is not MP4"
                return False

            total = len(prefix)
            with temporary.open("wb") as output:
                output.write(prefix)
                while True:
                    chunk = response.read(1024 * 1024)
                    if not chunk:
                        break
                    total += len(chunk)
                    if total > MAX_VIDEO_BYTES:
                        raise ValueError("SNS video exceeds the maximum allowed size")
                    output.write(chunk)
        temporary.replace(destination)
        media["local_file"] = f"videos/{destination.name}"
        media["video_source"] = "remote"
        media["video_complete"] = True
        media["video_bytes"] = destination.stat().st_size
        media.pop("video_error", None)
        return True
    except Exception as error:
        media["video_error"] = clean_text(error)
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
        return False


def save_image(data: bytes, destination: Path, media: dict) -> bool:
    extension = detect_image_extension(data)
    if not extension:
        return False
    destination = destination.with_suffix(f".{extension}")
    destination.write_bytes(data)
    media["local_file"] = f"images/{destination.name}"
    return True


def reuse_existing_image(destination: Path, media: dict) -> bool:
    for extension in ("jpg", "png", "gif", "webp"):
        candidate = destination.with_suffix(f".{extension}")
        try:
            with candidate.open("rb") as stream:
                header = stream.read(16)
        except OSError:
            continue
        if detect_image_extension(header) != extension:
            continue
        media["local_file"] = f"images/{candidate.name}"
        media["image_source"] = "existing"
        media.pop("image_error", None)
        return True
    return False


def reuse_existing_video(destination: Path, media: dict) -> bool:
    destination = destination.with_suffix(".mp4")
    try:
        with destination.open("rb") as stream:
            header = stream.read(16)
        if not is_mp4(header):
            return False
        media["local_file"] = f"videos/{destination.name}"
        media["video_source"] = "existing"
        media["video_complete"] = True
        media["video_bytes"] = destination.stat().st_size
        media.pop("video_error", None)
        return True
    except OSError:
        return False


def sns_image_url_candidates(url: str, token: str) -> list[str]:
    value = html.unescape(str(url or "")).strip()
    if not value:
        return []
    value = re.sub(r"^http://", "https://", value, flags=re.I)
    token = str(token or "").strip()

    def signed(candidate: str) -> str:
        if token and not re.search(r"(?:\?|&)token=", candidate, flags=re.I):
            connector = "&" if "?" in candidate else "?"
            candidate = f"{candidate}{connector}token={quote(token, safe='')}&idx=1"
        return candidate

    original_size = signed(value)
    full_size = signed(re.sub(r"/(?:150|200|480)(?=($|\?))", "/0", value))
    return list(dict.fromkeys((full_size, original_size)))


def fix_sns_image_url(url: str, token: str) -> str:
    candidates = sns_image_url_candidates(url, token)
    return candidates[0] if candidates else ""


def download_sns_image(
    url: str,
    key: str,
    token: str,
    destination: Path,
    media: dict,
    keystream: WasmKeystream,
) -> bool:
    headers = {
        "User-Agent": "MicroMessenger Client",
        "Accept": "*/*",
        "Referer": "https://mp.weixin.qq.com/",
    }
    fixed_urls = sns_image_url_candidates(url, token)
    if not fixed_urls:
        media["image_error"] = "missing image URL"
        return False
    errors = []
    for fixed_url in fixed_urls:
        try:
            with urlopen(Request(fixed_url, headers=headers), timeout=10) as response:
                data = response.read(25 * 1024 * 1024 + 1)
            if not data or len(data) > 25 * 1024 * 1024:
                errors.append("image response is empty or too large")
                continue
            encrypted = detect_image_extension(data) is None
            key = str(key or "").strip()
            if encrypted and key and key != "0":
                stream = keystream.generate(key, len(data))
                data = bytes(value ^ stream[index] for index, value in enumerate(data))
            if not save_image(data, destination, media):
                errors.append("decrypted image format is not supported")
                continue
            media["image_source"] = "remote_decrypted" if encrypted else "remote"
            media.pop("image_error", None)
            return True
        except Exception as error:
            errors.append(clean_text(error))
    media["image_error"] = "; ".join(errors)
    return False


def load_cache_helpers():
    import export_sns  # type: ignore

    video_root = Path(export_sns.XWECHAT_CACHE_DIR) if export_sns.XWECHAT_CACHE_DIR else None
    return build_video_cache_index(video_root)


def post_images(post: dict) -> list[dict]:
    return [
        media
        for media in post.get("media", [])
        if str(media.get("type")) == "2" and media.get("local_file")
    ]


def post_videos(post: dict) -> list[dict]:
    return [
        media
        for media in post.get("media", [])
        if str(media.get("type")) in VIDEO_TYPES and media.get("local_file")
    ]


def readable_posts(posts: list[dict]) -> list[dict]:
    return [
        post
        for post in posts
        if clean_text(post.get("content")) or post_images(post) or post_videos(post)
    ]


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


def video_tag(media: dict) -> str:
    src = html.escape(str(media["local_file"]), quote=True)
    return f'<video src="{src}" controls preload="metadata" playsinline></video>'


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
        ".year-nav a{flex:0 0 auto;color:#334155;text-decoration:none;padding:5px 10px;border-radius:6px;background:#eef2f7;font-size:14px}main{max-width:780px;margin:0 auto;padding:0 18px 52px}.year{scroll-margin-top:62px;margin:34px 0 8px;font-size:24px}.post{padding:20px 0;border-top:1px solid #e5e7eb}.time{color:#64748b;font-size:14px;margin-bottom:7px}.content{white-space:normal;font-size:16px;overflow-wrap:anywhere}.emoji{font-family:'Segoe UI Emoji','Apple Color Emoji',sans-serif}.imgs{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:7px;margin-top:13px}.imgs a{display:block;min-width:0}.imgs img{display:block;width:100%;height:auto;max-height:620px;object-fit:cover;background:#e5e7eb;border-radius:6px}.videos{display:grid;gap:10px;margin-top:13px}.videos video{display:block;width:100%;max-height:720px;background:#111;border-radius:6px}",
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
        videos = post_videos(post)
        if videos:
            parts.append('<div class="videos">')
            parts.extend(video_tag(media) for media in videos)
            parts.append("</div>")
        parts.append("</article>")
    parts.extend(["</main></body></html>"])
    output.write_text("\n".join(parts), encoding="utf-8", newline="\n")
    return len(visible)


def export_album(args: argparse.Namespace) -> dict:
    posts = run_wx_sns_feed(args.wx_exe, args.user, args.limit, args.since, args.until)
    run_id = datetime.now().strftime("%Y%m%d-%H%M%S")
    output_dir = args.output_dir or (
        args.output_root / f"{safe_stem(args.user, 'contact')}-朋友圈相册-{run_id}"
    )
    image_dir = output_dir / "images"
    video_dir = output_dir / "videos"
    image_dir.mkdir(parents=True, exist_ok=bool(args.output_dir))
    video_dir.mkdir(exist_ok=bool(args.output_dir))

    video_cache_index = load_cache_helpers()
    image_existing = 0
    image_cache = 0
    image_remote = 0
    image_missing = 0
    image_items = []
    video_items = []
    for post_index, post in enumerate(posts, 1):
        for media_index, media in enumerate(post.get("media", []), 1):
            media_type = str(media.get("type"))
            tid = safe_stem(post.get("tid"), str(post_index))
            if media_type in VIDEO_TYPES:
                video_items.append(
                    (post, media, video_dir / f"{post_index:05d}_{tid}_{media_index:02d}.mp4")
                )
                continue
            if media_type == "2":
                image_items.append(
                    (media, image_dir / f"{post_index:05d}_{tid}_{media_index:02d}.jpg")
                )

    image_workers = max(1, min(args.image_workers, 32))
    keystreams: queue.Queue[WasmKeystream] = queue.Queue()
    engines = [WasmKeystream() for _ in range(image_workers)]
    for engine in engines:
        keystreams.put(engine)

    def process_image(item: tuple[dict, Path]) -> str:
        media, destination = item
        if reuse_existing_image(destination, media):
            return "existing"
        if args.no_remote:
            media["image_error"] = "remote download disabled"
            return "missing"
        engine = keystreams.get()
        try:
            candidates = [
                (media.get("url"), media.get("url_key"), media.get("url_token")),
                (media.get("thumb"), media.get("thumb_key"), media.get("thumb_token")),
            ]
            for url, key, token in candidates:
                if download_sns_image(url, key, token, destination, media, engine):
                    return "remote"
            return "missing"
        finally:
            keystreams.put(engine)

    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=image_workers) as executor:
            for image_index, result in enumerate(executor.map(process_image, image_items), 1):
                if result == "existing":
                    image_existing += 1
                elif result == "remote":
                    image_remote += 1
                else:
                    image_missing += 1
                if image_index % 250 == 0 or image_index == len(image_items):
                    print(
                        f"  图片 {image_index}/{len(image_items)}: "
                        f"已有 {image_existing}, 下载 {image_remote}, 缺失 {image_missing}",
                        file=sys.stderr,
                        flush=True,
                    )
    finally:
        for engine in engines:
            engine.close()

    video_existing = 0
    video_cache = 0
    video_remote = 0
    video_partial_cache = 0
    video_missing = 0
    if not args.no_videos:
        video_workers = max(1, min(args.video_workers, 16))
        video_keystreams: queue.Queue[WasmKeystream] = queue.Queue()
        video_engines = [WasmKeystream() for _ in range(video_workers)]
        for engine in video_engines:
            video_keystreams.put(engine)

        def process_video(item: tuple[dict, dict, Path]) -> str:
            post, media, destination = item
            if reuse_existing_video(destination, media):
                return "existing"
            cached = find_cached_video(post, media, video_cache_index)
            cache_result = (
                copy_cached_video(cached, destination, media, allow_partial=False)
                if cached else None
            )
            if cache_result == "complete":
                return "cache"

            downloaded = False
            if not args.no_remote:
                engine = video_keystreams.get()
                try:
                    downloaded = download_decrypt_video(
                        str(media.get("url") or "").strip(),
                        str(media.get("enc_key") or "").strip(),
                        destination,
                        media,
                        engine,
                    )
                finally:
                    video_keystreams.put(engine)
            if downloaded:
                return "remote"

            cache_result = (
                copy_cached_video(cached, destination, media, allow_partial=True)
                if cached else None
            )
            return "partial" if cache_result == "partial" else "missing"

        try:
            with concurrent.futures.ThreadPoolExecutor(max_workers=video_workers) as executor:
                for video_index, result in enumerate(executor.map(process_video, video_items), 1):
                    if result == "existing":
                        video_existing += 1
                    elif result == "cache":
                        video_cache += 1
                    elif result == "remote":
                        video_remote += 1
                    elif result == "partial":
                        video_partial_cache += 1
                    else:
                        video_missing += 1
                    if video_index % 25 == 0 or video_index == len(video_items):
                        print(
                            f"  视频 {video_index}/{len(video_items)}: 已有 {video_existing}, "
                            f"缓存 {video_cache}, 下载 {video_remote}, "
                            f"部分 {video_partial_cache}, 缺失 {video_missing}",
                            file=sys.stderr,
                            flush=True,
                        )
        finally:
            for engine in video_engines:
                engine.close()

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
        "image_ok": image_existing + image_cache + image_remote,
        "image_existing": image_existing,
        "image_cache": image_cache,
        "image_remote": image_remote,
        "image_missing": image_missing,
        "video_total": len(video_items),
        "video_ok": video_existing + video_cache + video_remote + video_partial_cache,
        "video_complete": video_existing + video_cache + video_remote,
        "video_existing": video_existing,
        "video_cache": video_cache,
        "video_remote": video_remote,
        "video_partial_cache": video_partial_cache,
        "video_missing": video_missing,
        "video_skipped": len(video_items) if args.no_videos else 0,
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
    parser = argparse.ArgumentParser(description="Export one contact's Moments text, images, and videos.")
    parser.add_argument("--wx-exe", type=Path, required=True)
    parser.add_argument("--user", required=True)
    output_group = parser.add_mutually_exclusive_group(required=True)
    output_group.add_argument("--output-root", type=Path)
    output_group.add_argument("--output-dir", type=Path)
    parser.add_argument("--limit", type=int, default=5000)
    parser.add_argument("--image-workers", type=int, default=8)
    parser.add_argument("--video-workers", type=int, default=4)
    parser.add_argument("--since")
    parser.add_argument("--until")
    parser.add_argument("--no-remote", action="store_true")
    parser.add_argument("--no-videos", action="store_true")
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
