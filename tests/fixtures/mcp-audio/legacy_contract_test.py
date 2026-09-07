"""只执行旧服务的选定 AST 函数体，不导入服务、不发现账号、不解码或联网。"""
import ast
import contextlib
from datetime import datetime
import io
from pathlib import Path
import os
import sys
import threading
import types
import unittest
from unittest.mock import Mock, patch


SOURCE = Path(__file__).resolve().parents[3] / "vendor/wechat-decrypt/mcp_server.py"
TREE = ast.parse(SOURCE.read_text(encoding="utf-8-sig"))
FUNCTIONS = {"decode_voice", "transcribe_voice", "_save_voice_transcription_cache"}
NODES = [node for node in TREE.body if isinstance(node, ast.FunctionDef) and node.name in FUNCTIONS]
for node in NODES:
    node.decorator_list = []
CODE = compile(ast.Module(body=NODES, type_ignores=[]), str(SOURCE), "exec")


class LegacyContract(unittest.TestCase):
    def setUp(self):
        self.cache = {}
        self.fetch = Mock(return_value=(b"synthetic", 123))
        self.wav = Mock(return_value=("synthetic.wav", 48_000))
        self.transcribe = Mock(return_value={"text": "合成转录", "language": "zh"})
        self.env = {
            "resolve_username": lambda _: "wxid_peer",
            "_fetch_voice_row": self.fetch,
            "_silk_to_wav": self.wav,
            "_cache_signature": lambda: {"backend": "local", "model_size": "base"},
            "_voice_transcription_cache_key": lambda *_: "key",
            "_load_voice_transcription_cache": lambda: self.cache,
            "_transcribe": self.transcribe,
            "datetime": datetime,
            "os": types.SimpleNamespace(path=types.SimpleNamespace(getsize=lambda _: 48_044)),
        }
        exec(CODE, self.env)
        self.save = Mock()
        self.real_save = self.env["_save_voice_transcription_cache"]
        self.env["_save_voice_transcription_cache"] = self.save
        self.modules = patch.dict(sys.modules, {"pysilk": types.ModuleType("pysilk"), "whisper": types.ModuleType("whisper")})
        self.modules.start()
        self.addCleanup(self.modules.stop)

    def test_decode_success_exact_text(self):
        self.assertEqual(self.env["decode_voice"]("peer", 7),
                         "解码成功!\n  文件: synthetic.wav\n  时长: 1.0秒\n  大小: 48,044 bytes")

    def test_decode_dependency_precedes_contact_and_missing_messages(self):
        with patch.dict(sys.modules, {"pysilk": None}):
            self.assertEqual(self.env["decode_voice"]("peer", 7), "缺少依赖: pip install silk-python")
        self.env["resolve_username"] = lambda _: None
        self.assertEqual(self.env["decode_voice"]("peer", 7), "找不到聊天对象: peer")
        self.env["resolve_username"] = lambda _: "wxid_peer"
        self.fetch.return_value = None
        self.assertEqual(self.env["decode_voice"]("peer", 7), "找不到 local_id=7 的语音消息")

    def test_decode_exception_is_not_a_success_or_returned_error_string(self):
        self.wav.side_effect = OSError("synthetic publish failure")
        with self.assertRaises(OSError):
            self.env["decode_voice"]("peer", 7)

    def test_transcribe_success_exact_text_and_cache_fields(self):
        label = datetime.fromtimestamp(123).strftime("%Y-%m-%d %H:%M")
        self.assertEqual(self.env["transcribe_voice"]("peer", 7), f"[{label}] (zh)\n合成转录")
        self.assertEqual(self.cache["key"], {"text": "合成转录", "language": "zh", "create_time": 123, "backend": "local", "model_size": "base"})
        self.save.assert_called_once()

    def test_cached_empty_text_missing_timestamp_and_legacy_backend(self):
        self.cache["key"] = {"text": "", "model_size": "base"}
        with patch.dict(sys.modules, {"pysilk": None, "whisper": None}):
            self.assertEqual(self.env["transcribe_voice"]("peer", 7), "[-] (unknown)\n")
        self.fetch.assert_not_called()
        self.wav.assert_not_called()
        self.transcribe.assert_not_called()

    def test_transcribe_runtime_error_does_not_cache_but_wav_already_created(self):
        self.transcribe.side_effect = RuntimeError("synthetic backend failure")
        self.assertEqual(self.env["transcribe_voice"]("peer", 7), "synthetic backend failure")
        self.wav.assert_called_once()
        self.save.assert_not_called()
        self.assertEqual(self.cache, {})

    def test_transcribe_dependency_and_missing_errors(self):
        with patch.dict(sys.modules, {"whisper": None}):
            self.assertEqual(self.env["transcribe_voice"]("peer", 7), "缺少依赖: pip install openai-whisper")
        with patch.dict(sys.modules, {"pysilk": None}):
            self.assertEqual(self.env["transcribe_voice"]("peer", 7), "缺少依赖: pip install silk-python")
        self.fetch.return_value = None
        self.assertEqual(self.env["transcribe_voice"]("peer", 7), "找不到 local_id=7 的语音消息")

    def test_cache_write_failure_keeps_success_and_in_memory_entry(self):
        self.env.update({"os": os, "sys": sys, "_voice_transcription_cache": self.cache,
                         "_voice_transcription_cache_lock": threading.Lock(),
                         "_voice_transcription_save_warned": False,
                         "VOICE_TRANSCRIPTION_CACHE_FILE": "synthetic-unused-cache.json",
                         "_save_voice_transcription_cache": self.real_save})
        with patch("builtins.open", side_effect=OSError("synthetic read-only")), patch("os.path.exists", return_value=False), contextlib.redirect_stderr(io.StringIO()) as stderr:
            result = self.env["transcribe_voice"]("peer", 7)
        self.assertTrue(result.endswith("\n合成转录"))
        self.assertEqual(self.cache["key"]["text"], "合成转录")
        self.assertIn("[voice_cache]", stderr.getvalue())


if __name__ == "__main__":
    unittest.main(verbosity=2)
