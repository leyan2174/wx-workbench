"""仅用合成缓存，AST 提取旧 SNS 图片/视频本地函数并生成差分回归；不加载配置或 keys。"""
from __future__ import annotations

import ast
import bisect
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import sys
import tempfile
import zlib

from Crypto.Cipher import AES
from Crypto.Util.Padding import pad

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "tests" / "fixtures" / "sns" / "cache_golden.json"
KEY = b"0123456789ABCDEF"  # 固定合成测试 key，不来自任何用户账号。
STAMP = 1700000000


def extract(namespace, relative, functions, constants=()):
    path = ROOT / relative
    tree = ast.parse(path.read_text(encoding="utf-8-sig"))
    nodes = []
    found = set()
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name in functions:
            nodes.append(node)
            found.add(node.name)
        elif isinstance(node, ast.Assign) and all(isinstance(t, ast.Name) and t.id in constants for t in node.targets):
            nodes.append(node)
    assert found == set(functions), (relative, found)
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(path), "exec"), namespace)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def legacy():
    namespace = dict(os=os, struct=struct, bisect=bisect, Path=Path, re=re, shutil=shutil, hashlib=hashlib)
    hashes = {}
    hashes["decode_image.py"] = extract(namespace, "vendor/wechat-decrypt/decode_image.py", {"aligned_aes_block_size"})
    hashes["export_sns.py"] = extract(namespace, "vendor/wechat-decrypt/export_sns.py", {
        "_decrypt_sns_dat", "_detect_format", "_image_size_from_bytes", "_build_sns_cache_index", "_match_cache_images",
    }, {"_V1_MAGIC", "_V2_MAGIC", "_IMAGE_MAGICS", "_TIME_WINDOW"})
    hashes["export_sns_album.py"] = extract(namespace, "vendor/wechat-decrypt/export_sns_album.py", {
        "is_mp4", "video_cache_key", "build_video_cache_index", "find_cached_video", "copy_cached_video", "detect_image_extension", "save_image",
    })
    namespace.update(IMAGE_AES_KEY=KEY, IMAGE_XOR_KEY=0x88)
    return namespace, hashes


def png(width, height, extra=0):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"tEXt", b"synthetic\0" + b"x" * extra)
            + chunk(b"IDAT", zlib.compress((b"\0" + b"\x11\x22\x33" * width) * height)) + chunk(b"IEND", b""))


def dat(plain, aes_size, tail=0, version=2, xor_key=0x88):
    assert aes_size + tail <= len(plain)
    key = KEY if version == 2 else b"cfcd208495d565ef"
    magic = b"\x07\x08V2\x08\x07" if version == 2 else b"\x07\x08V1\x08\x07"
    encrypted = AES.new(key, AES.MODE_ECB).encrypt(pad(plain[:aes_size], 16))
    middle = plain[aes_size:len(plain) - tail] if tail else plain[aes_size:]
    ending = bytes(b ^ xor_key for b in plain[-tail:]) if tail else b""
    return magic + struct.pack("<II", aes_size, tail) + b"\0" + encrypted + middle + ending


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    old, hashes = legacy()
    plain = png(8, 6)
    decrypt_cases, files, matches, lookups, copies = [], [], [], [], []
    with tempfile.TemporaryDirectory(prefix="wx-sns-cache-golden-") as directory:
        root = Path(directory)
        xwechat, storage = root / "xwechat", root / "storage"
        xwechat.mkdir()
        storage.mkdir()
        old.update(XWECHAT_CACHE_DIR=str(xwechat), SNS_CACHE_DIR=str(storage))

        def decrypt_case(name, encoded, key=KEY, xor_key=0x88, strict_reject=False):
            old.update(IMAGE_AES_KEY=key, IMAGE_XOR_KEY=xor_key)
            source = root / "synthetic.dat"
            source.write_bytes(encoded)
            output = old["_decrypt_sns_dat"](str(source))
            decrypt_cases.append(dict(name=name, input_hex=encoded.hex(), key_hex=key.hex() if key else None,
                                      xor_key=xor_key, expected_hex=output.hex() if output is not None else None,
                                      strict_reject=strict_reject))

        for version in (1, 2):
            for size in (0, 1, 15, 16, 17, 32, len(plain)):
                tail = min(7, len(plain) - size)
                decrypt_case(f"v{version}_aes_{size}", dat(plain, size, tail, version), key=KEY if version == 2 else None)
        decrypt_case("v2_custom_xor", dat(plain, 32, 9, xor_key=0x37), xor_key=0x37)
        decrypt_case("v2_missing_key", dat(plain, 32, 9), key=None)
        decrypt_case("v2_wrong_key", dat(plain, 32, 9), key=b"fedcba9876543210")
        decrypt_case("v1_ignores_v2_key", dat(plain, 32, 9, version=1), key=b"fedcba9876543210")
        bad_padding = bytearray(dat(plain, 32, 9))
        bad_padding[15 + 47] ^= 0x11
        decrypt_case("bad_padding", bytes(bad_padding))
        decrypt_case("truncated_aes", dat(plain, 32, 9)[:30])
        overlap = bytearray(dat(plain, 32, 9))
        overlap[10:14] = struct.pack("<I", len(overlap) - 16)
        decrypt_case("strict_overlap", bytes(overlap), strict_reject=True)
        mismatch = bytearray(dat(plain, 32, 9))
        mismatch[6:10] = struct.pack("<I", 33)
        decrypt_case("strict_plaintext_length", bytes(mismatch), strict_reject=True)
        jpeg = b"\xff\xd8\xff\xc0\x00\x11\x08" + struct.pack(">HH", 6, 8) + b"\0" * 24
        webp = b"RIFF" + struct.pack("<I", 32) + b"WEBPVP8 " + b"\0" * 10 + struct.pack("<HH", 8, 6) + b"\0" * 10
        gif = b"GIF89a" + struct.pack("<HH", 8, 6) + b"\0" * 30
        for name, payload in [("png", plain), ("jpg", jpeg), ("gif", gif), ("webp", webp)]:
            decrypt_case(f"xor_{name}", bytes(b ^ 0x5a for b in payload))
            decrypt_case(f"plain_{name}", payload)
        decrypt_case("too_short", b"\xff\xd8\xff")
        decrypt_case("unknown", b"unknown-synthetic-data")
        old.update(IMAGE_AES_KEY=KEY, IMAGE_XOR_KEY=0x88)

        def add(relative, payload, stamp=STAMP):
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payload)
            os.utime(path, (stamp, stamp))
            files.append(dict(path=relative, input_hex=payload.hex(), mtime=stamp))
            return path

        add("xwechat/2023-11/Sns/Img/aa/v1", dat(plain, 48, 7, version=1), STAMP + 60)
        add("xwechat/2023-11/Sns/Img/ab/v2", dat(png(8, 6, 20), 32, 9), STAMP + 1)
        add("storage/2023-11/xor", bytes(b ^ 0x5a for b in png(8, 6, 40)), STAMP + 3)
        add("storage/2023-11/xor_t", bytes(b ^ 0x5a for b in plain), STAMP)
        add("storage/2023-12/plain", png(3, 4), STAMP + 10 * 86400)
        add("xwechat/2023-11/Other/Img/aa/not-scanned", dat(plain, 32), STAMP)
        add("xwechat/2023-11/Sns/Img/ac/invalid", b"not-an-image" * 3, STAMP)
        corrupt = bytearray(dat(png(33, 44), 32, 9))
        corrupt[15 + 47] ^= 0x11
        add("xwechat/2023-11/Sns/Img/ad/bad-padding", bytes(corrupt), STAMP + 5)
        add("xwechat/2023-11/Sns/Img/ae/tie-first", dat(png(11, 12), 32, 9), STAMP + 2)
        add("xwechat/2023-11/Sns/Img/af/tie-second", dat(png(11, 12), 32, 9), STAMP + 2)
        mp4 = b"\0\0\0\x18ftypisom" + b"synthetic-video" * 5
        video_specs = [
            ("18446744073709551610", "12345678901234567890", ".mp4", mp4),
            ("-7", "partial-media", ".tmp", mp4),
            ("0", "zero-media", ".mp4", mp4),
            ("18446744073709551610", "bad-header", ".mp4", b"not-mp4" * 8),
            ("duplicate-post", "larger-partial", ".mp4", mp4),
            ("duplicate-post", "later-complete", ".tmp", mp4 * 2),
        ]
        for post_id, media_id, extension, body in video_specs:
            digest = old["video_cache_key"](post_id, media_id)
            add(f"xwechat/2023-11/Sns/Video/{digest[:2]}/{digest[2:]}{extension}", body)
        digest = old["video_cache_key"]("duplicate-post", "larger-partial")
        add(f"xwechat/2023-12/Sns/Video/{digest[:2]}/{digest[2:]}.tmp", mp4 * 2)
        digest = old["video_cache_key"]("duplicate-post", "later-complete")
        add(f"xwechat/2024-01/Sns/Video/{digest[:2]}/{digest[2:]}.MP4", mp4)
        add("xwechat/2023-11/Sns/Video/xx/not-a-hash.mp4", mp4)
        image_index = old["_build_sns_cache_index"]()
        video_index = old["build_video_cache_index"](xwechat)

        def relative(path):
            return Path(path).relative_to(root).as_posix() if path else None

        expected_images = []
        for stamp, path, size, extension, width, height in image_index:
            restored = old["_decrypt_sns_dat"](path)
            expected_images.append(dict(path=relative(path), mtime=stamp, estimated_size=size,
                                        format=extension, width=width, height=height,
                                        decoded_hex=restored.hex() if restored is not None else None))
        for name, stamp, media in [
            ("dimensions_and_size", STAMP, [{"type": "2", "width": "8", "height": "6", "total_size": str(len(plain))}]),
            ("unique_per_post", STAMP, [{"type": "2", "width": "8", "height": "6"}] * 4),
            ("fallback_empty_window", STAMP - 30 * 86400, [{"type": "2", "width": "3", "height": "4"}]),
            ("no_fallback_for_dimension_miss", STAMP, [{"type": "2", "width": "3", "height": "4"}]),
            ("size_ratio_reject", STAMP, [{"type": "2", "total_size": "9999999"}]),
            ("non_image_and_missing_type", STAMP, [{"type": "6"}, {"type": "15"}, {"type": "28"}, {}]),
            ("lower_window_boundary", STAMP + 60 + 72 * 3600, [{"type": "2", "width": "8", "height": "6"}]),
            ("upper_window_boundary", STAMP + 1 - 72 * 3600, [{"type": "2", "width": "8", "height": "6"}]),
            ("stable_tie_order", STAMP, [{"type": "2", "width": "11", "height": "12"}] * 2),
        ]:
            result = old["_match_cache_images"](stamp, media, image_index, [e[0] for e in image_index])
            matches.append(dict(name=name, create_time=stamp, media=media, expected_paths=[relative(p) for p, _ in result]))
        for post, media in [
            ({"post_id": "18446744073709551610", "tid": -6}, {"id": "12345678901234567890"}),
            ({"tid": -6}, {"id": "12345678901234567890"}),
            ({"tid": -7}, {"id": "partial-media"}),
            ({"tid": 0}, {"id": "zero-media"}),
            ({"tid": -6}, {}),
            ({"tid": -6}, {"id": "unavailable"}),
            ({"post_id": "duplicate-post"}, {"id": "larger-partial"}),
            ({"post_id": "duplicate-post"}, {"id": "later-complete"}),
        ]:
            found = old["find_cached_video"](post, media, video_index)
            lookups.append(dict(post=post, media=media, expected_path=relative(found)))
        for number, (post_id, media_id, _, _) in enumerate(video_specs):
            source = video_index[old["video_cache_key"](post_id, media_id)]
            for allow in (False, True):
                destination = root / "out" / "videos" / f"copy_{number}_{int(allow)}.mp4"
                destination.parent.mkdir(parents=True, exist_ok=True)
                media = {}
                status = old["copy_cached_video"](source, destination, media, allow)
                copies.append(dict(source=relative(source), post_id=post_id, media_id=media_id, allow_partial=allow,
                                   stem=f"copy_{number}_{int(allow)}", legacy_status=status, expected_reference=media,
                                   expected_hex=destination.read_bytes().hex() if destination.exists() else None))
        dimensions = [dict(input_hex=data.hex(), format=old["_detect_format"](data),
                           dimensions=old["_image_size_from_bytes"](data)) for data in (plain, jpeg, gif, webp, b"short")]
        payload = dict(provenance=dict(synthetic_only=True, source_sha256=hashes), key_hex=KEY.hex(),
                       decrypt_cases=decrypt_cases, files=files, expected_images=expected_images,
                       expected_videos={key: relative(path) for key, path in video_index.items()},
                       matches=matches, video_lookups=lookups, video_copies=copies, dimensions=dimensions)
    DEST.parent.mkdir(parents=True, exist_ok=True)
    DEST.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"Generated {len(decrypt_cases)} DAT cases, {len(files)} synthetic files, {len(matches)} matching cases, {len(copies)} video copy cases")
    print(f"Golden: {DEST}")
    print("No private cache, config, keys, network, Node, or WASM accessed.")


if __name__ == "__main__":
    main()
