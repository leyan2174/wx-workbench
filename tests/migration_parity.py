"""Development-only golden comparison. Python is not a native command dependency."""
import argparse
import contextlib
import importlib
import io
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile

from Crypto.Cipher import AES
from Crypto.Util.Padding import pad


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--wx', type=Path, required=True)
    parser.add_argument('--account-config', type=Path)
    args = parser.parse_args()
    wx = args.wx.resolve()
    vendor = Path(__file__).resolve().parents[1] / 'vendor' / 'wechat-decrypt'
    sys.path.insert(0, str(vendor))
    with tempfile.TemporaryDirectory(prefix='wx-rust-parity-') as temp:
        root = Path(temp)
        source = root / 'db_storage'
        source.mkdir()
        aes_key = b'0123456789abcdef'
        cfg = {'db_dir': str(source), 'keys_file': 'all_keys.json',
               'decrypted_dir': str(root / 'rust-db'),
               'image_aes_key': aes_key.decode(), 'image_xor_key': 0x88}
        (root / 'config.json').write_text(json.dumps(cfg), encoding='utf-8')
        os.environ['WECHAT_DECRYPT_APP_DIR'] = str(root)
        env = dict(os.environ, WX_CLI_HOME=str(root / 'runtime'), WX_CLI_CONFIG=str(root / 'config.json'),
                   WX_WECHAT_DECRYPT_PYTHON=str(root / 'nonexistent-python.exe'))

        def run(*argv, success=True):
            result = subprocess.run([str(wx), *argv], cwd=root, env=env,
                                    capture_output=True, text=True, encoding='utf-8', timeout=90)
            if (result.returncode == 0) != success:
                raise AssertionError(f'Command status mismatch: {argv[0:2]}: {result.stderr}')
            return result

        legacy = importlib.import_module('decode_image')
        plain = b'\x89PNG\r\n\x1a\n' + bytes(range(256)) + b'\0\0\0\0IEND\xaeB\x60\x82'
        images = root / 'input'
        images.mkdir()
        fixtures = {'xor': bytes(b ^ 0x37 for b in plain)}
        for label, magic, key in [('v1', b'\x07\x08V1\x08\x07', b'cfcd208495d565ef'),
                                  ('v2', b'\x07\x08V2\x08\x07', aes_key)]:
            aes_size, xor_size = 32, 40
            fixtures[label] = (magic + struct.pack('<II', aes_size, xor_size) + b'\0'
                               + AES.new(key, AES.MODE_ECB).encrypt(pad(plain[:aes_size], 16))
                               + plain[aes_size:-xor_size]
                               + bytes(b ^ 0x88 for b in plain[-xor_size:]))
        for label, data in fixtures.items():
            dat = images / f'{label}.part.dat'
            dat.write_bytes(data)
            legacy_out = root / f'legacy-{label}.png'
            rust_out = root / f'rust-{label}.png'
            diagnostic = io.StringIO()
            with contextlib.redirect_stdout(diagnostic):
                result, _ = legacy.decrypt_dat_file(str(dat), str(legacy_out), aes_key, 0x88)
            assert result is not None, f'Legacy fixture {label}: {diagnostic.getvalue()}'
            run('toolkit', 'decode-image', str(dat), str(rust_out))
            assert Path(result).read_bytes() == rust_out.read_bytes() == plain
        print('PASS: XOR/V1/V2 Python-Rust byte equivalence (3 fixtures)')
        run('toolkit', 'batch-decrypt-images', str(images), str(root / 'batch'))
        for label in fixtures:
            assert (root / 'batch' / f'{label}.part.png').read_bytes() == plain
        print('PASS: recursive image batch, dotted filenames, no Python fallback')

        batch_input = root / 'batch-input' / 'nested'
        batch_input.mkdir(parents=True)
        for name in ['photo.part.dat', 'photo.part_h.dat', 'thumbnail_t.dat']:
            (batch_input / name).write_bytes(fixtures['v2'])
        legacy_batch = root / 'legacy-batch'
        legacy_result = subprocess.run([sys.executable, str(vendor / 'batch_decrypt_images.py'),
                                       str(batch_input.parent), str(legacy_batch)], env=os.environ,
                                      capture_output=True, text=True, encoding='utf-8', timeout=90)
        assert legacy_result.returncode == 0, legacy_result.stderr
        native_batch = root / 'native-batch'
        summary = json.loads(run('toolkit', 'batch-decrypt-images', str(batch_input.parent),
                                 str(native_batch)).stdout)
        expected = {p.relative_to(legacy_batch): p.read_bytes() for p in legacy_batch.rglob('*') if p.is_file()}
        actual = {p.relative_to(native_batch): p.read_bytes() for p in native_batch.rglob('*') if p.is_file()}
        assert expected == actual and summary['written'] == 2 and summary['skipped'] == 1
        repeated = json.loads(run('toolkit', 'batch-decrypt-images', str(batch_input.parent),
                                  str(native_batch)).stdout)
        assert repeated['skipped'] == 3 and repeated['written'] == 0
        print('PASS: legacy batch output tree, thumbnail deduplication and repeated-run skip')

        layout = root / 'attach' / 'chat' / '2026-09' / 'Img'
        layout.mkdir(parents=True)
        (layout / 'hash_t.dat').write_bytes(fixtures['v2'])
        album_out = root / 'album'
        first = json.loads(run('toolkit', 'decode-images', '--attach-dir', str(root / 'attach'),
                               '--decoded-dir', str(album_out)).stdout)
        second = json.loads(run('toolkit', 'decode-images', '--attach-dir', str(root / 'attach'),
                                '--decoded-dir', str(album_out)).stdout)
        assert first['written'] == 1 and second['skipped'] == 1
        assert (album_out / 'chat/2026-09/hash.png').read_bytes() == plain
        run('toolkit', 'batch-decrypt-images', str(images), str(images / 'unsafe'), success=False)
        print('PASS: thumbnail layout, incremental image skip and source-path guard')

        prior = (album_out / 'chat/2026-09/hash.png').read_bytes()
        run('toolkit', 'decode-images', '--attach-dir', str(root / 'attach'),
            '--decoded-dir', str(album_out), '--xor-key', '0x77', '--force', success=False)
        assert (album_out / 'chat/2026-09/hash.png').read_bytes() == prior
        with contextlib.redirect_stdout(io.StringIO()):
            rejected, _ = legacy.decrypt_dat_file(str(layout / 'hash_t.dat'),
                                                  str(root / 'invalid.png'), aes_key, 0x77)
        assert rejected is None
        print('PASS: wrong XOR rejected by both engines; previous output preserved')

        if args.account_config:
            account = json.loads(args.account_config.read_text(encoding='utf-8-sig'))
            keys_path = Path(account.get('keys_file', 'all_keys.json'))
            if not keys_path.is_absolute():
                keys_path = args.account_config.parent / keys_path
            keys = json.loads(keys_path.read_text(encoding='utf-8-sig'))
            rel = 'session/session.db'
            entry = keys.get(rel) or keys.get(rel.replace('/', '\\'))
            assert entry, 'No session database key in supplied account'
            (source / 'session').mkdir()
            account_db = Path(account['db_dir'])
            if not account_db.is_absolute():
                account_db = args.account_config.parent / account_db
            shutil.copyfile(account_db / rel, source / rel)
            (root / 'all_keys.json').write_text(json.dumps({rel: entry}), encoding='utf-8')
            planned = json.loads(run('toolkit', 'decrypt', '--dry-run').stdout)
            assert planned['planned'] == 1 and not (root / 'rust-db').exists()
            run('toolkit', 'decrypt')
            old = importlib.import_module('decrypt_db')
            key = bytes.fromhex(entry['enc_key'] if isinstance(entry, dict) else entry)
            with contextlib.redirect_stdout(io.StringIO()):
                ok = old.decrypt_database(str(source / rel), str(root / 'legacy.db'), key)
            assert ok
            assert (root / 'legacy.db').read_bytes() == (root / 'rust-db' / rel).read_bytes()
            incremental = json.loads(run('toolkit', 'decrypt', '--incremental').stdout)
            assert incremental['skipped'] == 1
            before = (root / 'rust-db' / rel).read_bytes()
            (root / 'all_keys.json').write_text(json.dumps({rel: {'enc_key': '00' * 32}}), encoding='utf-8')
            run('toolkit', 'decrypt', success=False)
            assert (root / 'rust-db' / rel).read_bytes() == before
            print('PASS: database byte equivalence, dry-run, incremental skip and wrong-key preservation')
        else:
            print('SKIP: private database comparison; pass --account-config to enable')
    print('Migration parity checks complete; temporary fixtures removed')


if __name__ == '__main__':
    main()
