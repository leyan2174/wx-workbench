import tempfile
import unittest
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
