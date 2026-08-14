"""Convert one SILK file to MP3 for wx-cli toolkit smoke-safe usage."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    import pilk
except ImportError:
    print("[ERROR] missing pilk; install it in wechat-decrypt-env", file=sys.stderr)
    raise


def convert_silk_to_mp3(input_path: Path, output_path: Path) -> None:
    if not input_path.is_file():
        raise FileNotFoundError(input_path)
    if not shutil.which("ffmpeg"):
        raise RuntimeError("ffmpeg is not in PATH")

    data = input_path.read_bytes()
    if data[:1] == b"\x02":
        data = data[1:]
    if not data.startswith(b"#!SILK_V3"):
        raise RuntimeError(f"not a SILK_V3 file: {input_path}")
    if not data.endswith(b"\xff\xff"):
        data += b"\xff\xff"

    output_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        silk_path = Path(tmp) / "input.silk"
        pcm_path = Path(tmp) / "output.pcm"
        silk_path.write_bytes(data)
        pilk.decode(str(silk_path), str(pcm_path))
        result = subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-f",
                "s16le",
                "-ar",
                "24000",
                "-ac",
                "1",
                "-i",
                str(pcm_path),
                str(output_path),
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip() or "ffmpeg failed")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Convert one WeChat SILK file to MP3.")
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args(argv)
    convert_silk_to_mp3(args.input, args.output)
    print({"input": str(args.input), "output": str(args.output), "size": os.path.getsize(args.output)})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
