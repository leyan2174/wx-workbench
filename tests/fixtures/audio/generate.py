"""仅测试：生成无真实语音的 SILK 与 pilk 差分基准。"""
import hashlib
import json
import math
from pathlib import Path
import struct
import tempfile

import pilk

ROOT = Path(__file__).resolve().parent


def main():
    manifest = {"generator": "pilk " + pilk.__version__, "sample_rate": 24000,
                "channels": 1, "sample_format": "s16le", "fixtures": []}
    for name, kind, packet_ms in [("tone", "tone", 20), ("silence", "silence", 20),
                                  ("sweep", "sweep", 20), ("multi40", "tone", 40),
                                  ("multi100", "tone", 100)]:
        samples = []
        for i in range(24000):
            phase = 2 * math.pi * (440 * i / 24000 if kind == "tone"
                                  else 100 * i / 24000 + 800 * (i / 24000) ** 2)
            samples.append(0 if kind == "silence" else round(10000 * math.sin(phase)))
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "source.pcm"
            source.write_bytes(struct.pack("<24000h", *samples))
            target = ROOT / (name + ".silk")
            pilk.SilkEncoder(pcm_rate=24000, silk_rate=24000, packet_size=packet_ms).encode(
                str(source), str(target), tencent=True)
            data = target.read_bytes()
            # 解码前移除可选的 Tencent 头标记，并补齐 SILK 结束标记。
            normalized = data[1:] if data[:1] == b"\x02" else data
            if not normalized.endswith(b"\xff\xff"):
                normalized += b"\xff\xff"
            silk = Path(temp) / "normalized.silk"
            silk.write_bytes(normalized)
            pcm = ROOT / (name + ".pcm")
            pilk.decode(str(silk), str(pcm))
            manifest["fixtures"].append({"name": name, "packet_ms": packet_ms,
                "silk_bytes": len(data), "pcm_bytes": pcm.stat().st_size,
                "silk_sha256": hashlib.sha256(data).hexdigest(),
                "pcm_sha256": hashlib.sha256(pcm.read_bytes()).hexdigest()})
    (ROOT / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
