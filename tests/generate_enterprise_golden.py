"""仅生成公开合成数据；用 vendor 原算法作为独立的 Rust 回归参照。"""

import hashlib
import importlib.util
import json
from contextlib import closing
from pathlib import Path
import sqlite3
import tempfile

from Crypto.Cipher import AES

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "tests" / "fixtures" / "enterprise"
KEY = bytes.fromhex("00112233445566778899aabbccddeeff")


def main():
    spec = importlib.util.spec_from_file_location(
        "enterprise_vendor_crypto", ROOT / "vendor" / "wechat-decrypt" / "wxwork_crypto.py"
    )
    legacy = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(legacy)
    OUT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="enterprise-synthetic-") as directory:
        path = Path(directory) / "synthetic.db"
        with closing(sqlite3.connect(path)) as db:
            db.execute("PRAGMA page_size=4096")
            db.execute("PRAGMA journal_mode=DELETE")
            db.execute("CREATE TABLE messages(id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
            db.executemany("INSERT INTO messages VALUES (?, ?)", [
                (i, f"synthetic enterprise message {i:04d}: " + "x" * 128)
                for i in range(1, 81)
            ])
            db.commit()
            assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
        plain = path.read_bytes()
    assert len(plain) > 4096 and len(plain) % 4096 == 0

    def encrypt(data, number):
        return AES.new(legacy.derive_wxsqlite3_aes128_page_key(KEY, number), AES.MODE_CBC,
                       legacy.generate_initial_vector(number)).encrypt(data)

    old, new = bytearray(), bytearray()
    for offset in range(0, len(plain), 4096):
        number = offset // 4096 + 1
        page = plain[offset:offset + 4096]
        encrypted = encrypt(page, number)
        old.extend(encrypted)
        if number == 1:
            fragment = page[16:24]
            encrypted = bytearray(encrypt(page[:16], number) + encrypt(page[16:], number))
            encrypted[8:16] = encrypted[16:24]
            encrypted[16:24] = fragment
        new.extend(encrypted)
        assert legacy.decrypt_wxsqlite3_aes128_page(KEY, encrypted, number) == page
        assert legacy.decrypt_wxsqlite3_aes128_page(KEY, old[offset:offset + 4096], number) == page
    files = {"plain.db": plain, "header.enc": new, "legacy.enc": old}
    manifest = {
        "synthetic_only": True, "raw_key_hex": KEY.hex(), "page_size": 4096,
        "pages": len(plain) // 4096, "rows": 80,
        "oracle": "vendor/wechat-decrypt/wxwork_crypto.py",
        "oracle_sha256": hashlib.sha256(Path(spec.origin).read_bytes()).hexdigest(),
        "vectors": [
            {"page": n, "key": legacy.derive_wxsqlite3_aes128_page_key(KEY, n).hex(),
             "iv": legacy.generate_initial_vector(n).hex()}
            for n in [1, 2, 256, 65536, 0xffffffff]
        ],
        "sha256": {name: hashlib.sha256(data).hexdigest() for name, data in files.items()},
    }
    for name, data in files.items():
        (OUT / name).write_bytes(data)
    with tempfile.TemporaryDirectory(prefix="enterprise-oracle-") as directory:
        for name in ["header.enc", "legacy.enc"]:
            decoded = Path(directory) / (name + ".db")
            legacy.decrypt_wxwork_database(str(OUT / name), str(decoded), KEY)
            assert decoded.read_bytes() == plain
            assert legacy.verify_sqlite_file(str(decoded)) == ["messages"]
            with closing(sqlite3.connect(decoded)) as db:
                assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
                assert db.execute("SELECT count(*) FROM messages").fetchone() == (80,)
    manifest["vendor_page_and_file_roundtrip"] = True
    (OUT / "golden.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(manifest, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
