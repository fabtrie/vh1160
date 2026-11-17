# /// script
# requires-python = ">=3.10"
# dependencies = ["resvg-py"]
# ///
"""Render app/assets/icon/*.svg into app/assets/icon/vh1160.ico.

24 px and below come from vh1160-small.svg (thicker strokes), larger sizes from
vh1160.svg. Every entry is stored as PNG, which Windows reads since Vista.

    uv run tools/make_icon.py
"""

import struct
from pathlib import Path

import resvg_py

ICON_DIR = Path(__file__).resolve().parent.parent / "app" / "assets" / "icon"
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]


def render(svg: Path, size: int) -> bytes:
    data = resvg_py.svg_to_bytes(svg_path=str(svg), width=size, height=size)
    # Older resvg-py releases return a list of ints.
    return bytes(data)


def main() -> None:
    entries = []
    for size in SIZES:
        svg = ICON_DIR / ("vh1160-small.svg" if size <= 24 else "vh1160.svg")
        png = render(svg, size)
        assert png[:8] == b"\x89PNG\r\n\x1a\n", f"{size}px: not a PNG"
        entries.append((size, png))

    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = len(header) + 16 * len(entries)
    directory, blobs = b"", b""
    for size, png in entries:
        dim = 0 if size >= 256 else size  # 0 means 256 in ICONDIR
        directory += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        blobs += png
        offset += len(png)
    out = ICON_DIR / "vh1160.ico"
    out.write_bytes(header + directory + blobs)
    (ICON_DIR / "vh1160-256.png").write_bytes(entries[-1][1])
    print(f"{out}: {', '.join(str(s) for s, _ in entries)} px, {out.stat().st_size} bytes")


if __name__ == "__main__":
    main()
