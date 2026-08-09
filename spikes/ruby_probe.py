"""スパイクA: NDLOCR-Lite が検出した「ルビ」BLOCK を切り出して PARSeq に通す。

NDLOCR-Lite 本体は BLOCK TYPE="ルビ" を bbox として検出するが、
LINE 要素にしないため文字認識に回さない（src/ocr.py の findall(".//LINE")）。
ここでは bbox から自前で切り出し、
  (1) ブロック全体を一括認識
  (2) 縦方向の空白でルビ群に分割して個別認識
の 2 通りを試し、解像度倍率ごとの精度を比較する。
"""

import sys
import xml.etree.ElementTree as ET
from pathlib import Path

import cv2
import numpy as np
from yaml import safe_load

BASE = Path(__file__).resolve().parent / "ndlocr-lite" / "src"
sys.path.insert(0, str(BASE))
from parseq import PARSEQ  # noqa: E402

MODELS = {
    "24x256": BASE / "model" / "parseq-ndl-24x256-30-tiny-189epoch-tegaki3-r8data-202604.onnx",
    "24x384": BASE / "model" / "parseq-ndl-24x384-50-tiny-300epoch-tegaki3-r8data-202604.onnx",
    "24x768": BASE / "model" / "parseq-ndl-24x768-100-tiny-153epoch-tegaki3-r8data-202604.onnx",
}


def load_charlist():
    with open(BASE / "config" / "NDLmoji.yaml", encoding="utf-8") as f:
        return list(safe_load(f)["model"]["charset_train"])


def split_ruby_groups(crop, min_gap_ratio=0.35):
    """縦書きルビ列を、縦方向の空白でルビ群に分割し (y0, y1) のリストを返す。

    ルビ群の間には必ず地の余白が入るため、暗画素の行方向プロファイルの
    ゼロ連続区間を区切りとして使う。min_gap_ratio は列幅に対する
    「区切りとみなす空白の最小長さ」の比。
    """
    gray = cv2.cvtColor(crop, cv2.COLOR_BGR2GRAY) if crop.ndim == 3 else crop
    dark = (gray < 160).sum(axis=1)  # 各行の暗画素数
    h, w = gray.shape
    min_gap = max(2, int(w * min_gap_ratio))

    groups, start, gap = [], None, 0
    for y in range(h):
        if dark[y] > 0:
            if start is None:
                start = y
            gap = 0
        else:
            if start is not None:
                gap += 1
                if gap >= min_gap:
                    groups.append((start, y - gap + 1))
                    start = None
    if start is not None:
        groups.append((start, h))
    # 1〜2px のノイズ塊を落とす
    return [(a, b) for a, b in groups if b - a >= max(3, w // 2)]


def main():
    scale = sys.argv[1]
    sp = Path(__file__).resolve().parent
    img_path = sp / "imgs" / f"ruby_x{scale}.png"
    xml_path = sp / f"out_x{scale}" / f"ruby_x{scale}.xml"

    img = cv2.imdecode(np.fromfile(str(img_path), dtype=np.uint8), cv2.IMREAD_COLOR)
    root = ET.parse(xml_path).getroot()
    charlist = load_charlist()
    rec_short = PARSEQ(model_path=str(MODELS["24x256"]), charlist=charlist, device="cpu")
    rec_long = PARSEQ(model_path=str(MODELS["24x768"]), charlist=charlist, device="cpu")

    blocks = [b for b in root.iter("BLOCK") if b.get("TYPE") == "ルビ"]
    # 縦書きは右から左に読むので X 降順
    blocks.sort(key=lambda b: -int(b.get("X")))

    print(f"===== x{scale}  画像 {img.shape[1]}x{img.shape[0]}  ルビBLOCK {len(blocks)}件 =====")
    for i, b in enumerate(blocks):
        x, y = int(b.get("X")), int(b.get("Y"))
        w, h = int(b.get("WIDTH")), int(b.get("HEIGHT"))
        pad = max(1, w // 8)
        x0, y0 = max(0, x - pad), max(0, y - pad)
        x1, y1 = min(img.shape[1], x + w + pad), min(img.shape[0], y + h + pad)
        crop = img[y0:y1, x0:x1]

        whole = rec_long.read(crop)
        groups = split_ruby_groups(crop)
        parts = []
        for a, bb in groups:
            g = crop[a:bb, :]
            if g.shape[0] < 3:
                continue
            parts.append(rec_short.read(g))

        print(f"\n[列{i}] bbox=({x},{y},{w}x{h}) conf={b.get('CONF')} 幅={w}px")
        print(f"  一括認識  : {whole}")
        print(f"  分割({len(parts):2d}群): {' / '.join(parts)}")


if __name__ == "__main__":
    main()
