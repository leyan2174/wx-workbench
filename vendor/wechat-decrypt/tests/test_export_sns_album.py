import io
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import export_sns_album


class ExportSnsAlbumTests(unittest.TestCase):
    def test_safe_stem_replaces_windows_reserved_characters(self):
        self.assertEqual(export_sns_album.safe_stem('a:b/c*?', 'fallback'), 'a_b_c__')

    def test_render_text_escapes_html_and_converts_known_emoji(self):
        rendered = export_sns_album.render_text('<b>[玫瑰]</b>')
        self.assertEqual(rendered, '&lt;b&gt;<span class="emoji">🌹</span>&lt;/b&gt;')

    def test_readable_posts_keeps_text_or_recovered_images(self):
        posts = [
            {"content": ""},
            {"content": "hello"},
            {"content": "", "media": [{"type": 2, "local_file": "images/a.jpg"}]},
        ]
        self.assertEqual(len(export_sns_album.readable_posts(posts)), 2)

    def test_readable_posts_keeps_recovered_videos(self):
        posts = [
            {"content": ""},
            {"content": "", "media": [{"type": 6, "local_file": "videos/a.mp4"}]},
        ]
        self.assertEqual(len(export_sns_album.readable_posts(posts)), 1)

    def test_build_html_omits_empty_posts_and_uses_relative_images(self):
        posts = [
            {"time": "2026-01-02 03:04", "content": "", "media": []},
            {
                "time": "2026-01-01 03:04",
                "content": "新年好[庆祝]",
                "media": [{"type": 2, "local_file": "images/a.jpg", "width": 800, "height": 600}],
            },
        ]
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "timeline.html"
            count = export_sns_album.build_html("测试用户", posts, output)
            page = output.read_text(encoding="utf-8")
        self.assertEqual(count, 1)
        self.assertIn("新年好", page)
        self.assertIn("🎉", page)
        self.assertIn('src="images/a.jpg"', page)
        self.assertNotIn("2026-01-02 03:04", page)

    def test_build_html_uses_relative_video(self):
        posts = [{
            "time": "2026-01-01 03:04",
            "content": "",
            "media": [{"type": 6, "local_file": "videos/a.mp4"}],
        }]
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "timeline.html"
            count = export_sns_album.build_html("测试用户", posts, output)
            page = output.read_text(encoding="utf-8")
        self.assertEqual(count, 1)
        self.assertIn('<video src="videos/a.mp4" controls', page)

    def test_video_cache_index_matches_post_and_media_ids(self):
        post_id = "18446744073709551610"
        media_id = "12345678901234567890"
        digest = export_sns_album.video_cache_key(post_id, media_id)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cached = root / "2026-01" / "Sns" / "Video" / digest[:2] / f"{digest[2:]}.mp4"
            cached.parent.mkdir(parents=True)
            cached.write_bytes(b"\0\0\0\x18ftypisom")
            index = export_sns_album.build_video_cache_index(root)
            match = export_sns_album.find_cached_video(
                {"post_id": post_id, "tid": -6},
                {"id": media_id},
                index,
            )
        self.assertEqual(match, cached)

    def test_download_decrypt_video_restores_mp4(self):
        plaintext = b"\0\0\0\x18ftypisom" + b"video-data" * 20
        encrypted = bytes(value ^ 0xAA for value in plaintext)

        class FakeResponse(io.BytesIO):
            headers = {"Content-Length": str(len(encrypted))}

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                self.close()

        class FakeKeystream:
            @staticmethod
            def generate(_key, size):
                return b"\xAA" * size

        media = {}
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "videos" / "sample.mp4"
            destination.parent.mkdir()
            with mock.patch.object(export_sns_album, "urlopen", return_value=FakeResponse(encrypted)):
                ok = export_sns_album.download_decrypt_video(
                    "https://example.invalid/video",
                    "123",
                    destination,
                    media,
                    FakeKeystream(),
                )
            restored = destination.read_bytes()
        self.assertTrue(ok)
        self.assertEqual(restored, plaintext)
        self.assertEqual(media["local_file"], "videos/sample.mp4")
        self.assertEqual(media["video_source"], "remote")

    def test_download_video_ignores_unreliable_xml_total_size(self):
        plaintext = b"\0\0\0\x18ftypisom" + b"video-data" * 20

        class FakeResponse(io.BytesIO):
            headers = {"Content-Length": str(len(plaintext))}

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                self.close()

        media = {"total_size": 12345}
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "videos" / "sample.mp4"
            destination.parent.mkdir()
            with mock.patch.object(export_sns_album, "urlopen", return_value=FakeResponse(plaintext)):
                ok = export_sns_album.download_decrypt_video(
                    "https://example.invalid/video",
                    "",
                    destination,
                    media,
                    mock.Mock(),
                )
            restored = destination.read_bytes()
        self.assertTrue(ok)
        self.assertEqual(restored, plaintext)

    def test_download_sns_image_decrypts_with_its_own_key(self):
        plaintext = b"\xff\xd8\xff" + b"image-data" * 20 + b"\xff\xd9"
        encrypted = bytes(value ^ 0xAA for value in plaintext)

        class FakeResponse(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                self.close()

        class FakeKeystream:
            @staticmethod
            def generate(_key, size):
                return b"\xAA" * size

        media = {}
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "images" / "sample.jpg"
            destination.parent.mkdir()
            with mock.patch.object(export_sns_album, "urlopen", return_value=FakeResponse(encrypted)) as opened:
                ok = export_sns_album.download_sns_image(
                    "http://example.invalid/image/150",
                    "123",
                    "token-value",
                    destination,
                    media,
                    FakeKeystream(),
                )
            restored = destination.read_bytes()
            requested_url = opened.call_args.args[0].full_url
        self.assertTrue(ok)
        self.assertEqual(restored, plaintext)
        self.assertEqual(media["image_source"], "remote_decrypted")
        self.assertEqual(requested_url, "https://example.invalid/image/0?token=token-value&idx=1")

    def test_fix_sns_image_url_percent_encodes_token(self):
        fixed = export_sns_album.fix_sns_image_url(
            "http://example.invalid/image/150",
            "abc+/=",
        )
        self.assertEqual(
            fixed,
            "https://example.invalid/image/0?token=abc%2B%2F%3D&idx=1",
        )

    def test_download_sns_image_falls_back_to_original_size(self):
        image = b"\xff\xd8\xff" + b"image-data" * 20 + b"\xff\xd9"

        class FakeResponse(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *args):
                self.close()

        media = {}
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "images" / "sample.jpg"
            destination.parent.mkdir()
            with mock.patch.object(
                export_sns_album,
                "urlopen",
                side_effect=[RuntimeError("HTTP 400"), FakeResponse(image)],
            ) as opened:
                ok = export_sns_album.download_sns_image(
                    "http://example.invalid/image/150",
                    "0",
                    "token-value",
                    destination,
                    media,
                    mock.Mock(),
                )
            requested_urls = [call.args[0].full_url for call in opened.call_args_list]
        self.assertTrue(ok)
        self.assertEqual(
            requested_urls,
            [
                "https://example.invalid/image/0?token=token-value&idx=1",
                "https://example.invalid/image/150?token=token-value&idx=1",
            ],
        )


if __name__ == "__main__":
    unittest.main()
