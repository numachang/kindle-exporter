"""スパイクC-5: 実際の Kindle ページ画像で OCR 精度とルビ検出を検証する。

黒地白字を反転してから NDLOCR-Lite にかけ、
  - 本文行が何行取れるか
  - ルビ BLOCK が検出されるか
  - ルビ列を一括認識して読めるか
を解像度別に比較する。
"""

import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

import cv2
import numpy as np
from yaml import safe_load

SP = Path(__file__).resolve().parent
BASE = SP / "ndlocr-lite" / "src"
OUT = SP / "cdp_out"
sys.path.insert(0, str(BASE))


def invert(src, dst):
    img = cv2.imdecode(np.fromfile(str(src), dtype=np.uint8), cv2.IMREAD_COLOR)
    inv = 255 - img
    cv2.imencode(".png", inv)[1].tofile(str(dst))
    return inv.shape


def run_ocr(img_path, out_dir):
    out_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [sys.executable, "src/ocr.py", "--sourceimg", str(img_path), "--output", str(out_dir)],
        cwd=str(BASE.parent), check=True, capture_output=True,
    )
    return out_dir / f"{img_path.stem}.xml"


def report(xml_path, img_path, label):
    from parseq import PARSEQ

    with open(BASE / "config" / "NDLmoji.yaml", encoding="utf-8") as f:
        charlist = list(safe_load(f)["model"]["charset_train"])
    rec = PARSEQ(
        model_path=str(BASE / "model" / "parseq-ndl-24x768-100-tiny-153epoch-tegaki3-r8data-202604.onnx"),
        charlist=charlist, device="cpu")

    img = cv2.imdecode(np.fromfile(str(img_path), dtype=np.uint8), cv2.IMREAD_COLOR)
    root = ET.parse(xml_path).getroot()
    lines = [ln for ln in root.iter("LINE")]
    rubies = [b for b in root.iter("BLOCK") if b.get("TYPE") == "ルビ"]

    print(f"\n{'=' * 70}\n{label}  画像 {img.shape[1]}x{img.shape[0]}\n{'=' * 70}")
    print(f"  本文行 {len(lines)} 行 / ルビBLOCK {len(rubies)} 件")
    confs = [float(ln.get("CONF", 0)) for ln in lines]
    if confs:
        print(f"  行 CONF: 平均 {np.mean(confs):.3f} 最小 {min(confs):.3f}")

    print("\n  --- 本文（先頭5行）---")
    for ln in lines[:5]:
        print(f"    [{ln.get('TYPE')}] {ln.get('STRING')}")

    print(f"\n  --- ルビ列を一括認識（先頭6列）---")
    rubies.sort(key=lambda b: -int(b.get("X")))
    for b in rubies[:6]:
        x, y = int(b.get("X")), int(b.get("Y"))
        w, h = int(b.get("WIDTH")), int(b.get("HEIGHT"))
        pad = max(1, w // 8)
        crop = img[max(0, y - pad):min(img.shape[0], y + h + pad),
                   max(0, x - pad):min(img.shape[1], x + w + pad)]
        print(f"    幅{w:3d}px conf={b.get('CONF')} → {rec.read(crop)}")


def main():
    for label, name in [("低解像度（表示相当 1501x1692）", "page_native"),
                        ("高解像度（viewport拡大 2400x3375）", "page_w2000")]:
        src = OUT / f"{name}.png"
        if not src.exists():
            continue
        inv = OUT / f"{name}_inv.png"
        invert(src, inv)
        xml = run_ocr(inv, OUT / f"ocr_{name}")
        report(xml, inv, label)
    return 0


if __name__ == "__main__":
    sys.exit(main())
