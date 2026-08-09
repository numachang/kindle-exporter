"""現在のリーダー設定（明色テーマ・フォント拡大・ウィンドウ拡大）で品質を測り直す。

viewport を一切上書きせず、いまブラウザに表示されている状態のまま測る。
比較対象は ADR-0004 の表（暗色・既定フォント・viewport 1251x1278 で
画素/文字=29、行CONF=0.927）。
"""

import base64
import json
import subprocess
import sys
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

import cv2
import numpy as np
from websocket import create_connection
from yaml import safe_load

SP = Path(__file__).resolve().parent
BASE = SP / "ndlocr-lite" / "src"
OUT = SP / "cdp_out" / "now"
PORT = 9222
sys.path.insert(0, str(BASE))

GRAB = r"""
(async () => {
  const img = document.querySelector('.kg-full-page-img img') || document.querySelector('#kr-renderer img');
  if (!img) return JSON.stringify({ error: 'ページ画像が見つかりません' });
  if (!img.complete || !img.naturalWidth) await new Promise(r => { img.onload = r; setTimeout(r, 4000); });
  const c = document.createElement('canvas');
  c.width = img.naturalWidth; c.height = img.naturalHeight;
  c.getContext('2d').drawImage(img, 0, 0);
  const r = img.getBoundingClientRect();
  const rend = document.querySelector('#kr-renderer');
  return JSON.stringify({
    natural: [img.naturalWidth, img.naturalHeight],
    shown: [Math.round(r.width), Math.round(r.height)],
    viewport: [innerWidth, innerHeight],
    dpr: devicePixelRatio,
    themeClass: rend ? rend.className : null,
    pageLabel: (document.body.innerText.match(/[\d,]+\s*\/\s*[\d,]+\s*ページ/) || [null])[0],
    dataUrl: c.toDataURL('image/png'),
  });
})()
"""


class CDP:
    def __init__(self, url):
        self.ws = create_connection(url, suppress_origin=True, timeout=90)
        self._id = 0

    def send(self, method, params=None, session_id=None):
        self._id += 1
        m = {"id": self._id, "method": method, "params": params or {}}
        if session_id:
            m["sessionId"] = session_id
        self.ws.send(json.dumps(m))
        while True:
            r = json.loads(self.ws.recv())
            if "method" in r:
                continue
            if r.get("id") == self._id:
                if "error" in r:
                    raise RuntimeError(f"{method}: {r['error']}")
                return r.get("result", {})


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    ver = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/version"))
    tgts = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list"))
    page = next(t for t in tgts if t["type"] == "page" and "read.amazon" in t["url"])
    cdp = CDP(ver["webSocketDebuggerUrl"])
    sid = cdp.send("Target.attachToTarget", {"targetId": page["id"], "flatten": True})["sessionId"]
    cdp.send("Runtime.enable", session_id=sid)
    cdp.send("Page.enable", session_id=sid)

    fid = cdp.send("Page.getFrameTree", session_id=sid)["frameTree"]["frame"]["id"]
    ctx = cdp.send("Page.createIsolatedWorld",
                   {"frameId": fid, "worldName": "now", "grantUniveralAccess": True},
                   session_id=sid)["executionContextId"]
    r = cdp.send("Runtime.evaluate",
                 {"expression": GRAB, "contextId": ctx, "returnByValue": True, "awaitPromise": True},
                 session_id=sid)
    raw = r["result"].get("value")
    if not isinstance(raw, str):
        print(f"取得失敗: {json.dumps(r, ensure_ascii=False)[:300]}")
        return 1
    v = json.loads(raw)
    if v.get("error"):
        print(v["error"])
        return 1

    png = base64.b64decode(v["dataUrl"].split(",", 1)[1])
    src = OUT / "page.png"
    src.write_bytes(png)
    img = cv2.imdecode(np.frombuffer(png, np.uint8), cv2.IMREAD_COLOR)
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)

    print("=" * 66)
    print("いまのリーダー設定")
    print("=" * 66)
    print(f"  viewport      : {v['viewport']}  dpr={v['dpr']}")
    print(f"  画像原寸      : {v['natural']}   表示: {v['shown']}")
    print(f"  倍率(原寸/vp) : {v['natural'][0] / v['viewport'][0]:.2f}")
    print(f"  テーマ class  : {v['themeClass']}")
    print(f"  ページ表示    : {v['pageLabel']}")
    print(f"  輝度 mean     : {gray.mean():.1f} → ", end="")
    inverted = gray.mean() < 110
    print("黒地白字（反転が必要）" if inverted else "★白地黒字（反転不要）")

    ocr_src = src
    if inverted:
        ocr_src = OUT / "page_inv.png"
        cv2.imencode(".png", 255 - img)[1].tofile(str(ocr_src))

    od = OUT / "ocr"
    od.mkdir(exist_ok=True)
    subprocess.run([sys.executable, "src/ocr.py", "--sourceimg", str(ocr_src), "--output", str(od)],
                   cwd=str(BASE.parent), check=True, capture_output=True)
    root = ET.parse(od / f"{ocr_src.stem}.xml").getroot()
    lines = list(root.iter("LINE"))
    rubies = [b for b in root.iter("BLOCK") if b.get("TYPE") == "ルビ"]
    chars = sum(len(ln.get("STRING") or "") for ln in lines)
    confs = [float(ln.get("CONF", 0)) for ln in lines]
    widths = [int(ln.get("WIDTH")) for ln in lines if int(ln.get("HEIGHT")) > int(ln.get("WIDTH"))]
    px = float(np.median(widths)) if widths else 0

    print("\n" + "=" * 66)
    print("OCR 結果")
    print("=" * 66)
    print(f"  本文行        : {len(lines)}")
    print(f"  総文字数      : {chars}")
    print(f"  行 CONF       : 平均 {np.mean(confs):.3f}  最小 {min(confs):.3f}")
    print(f"  画素/文字     : {px:.0f}   ← ADR-0004 の実測は 28〜30")
    print(f"  ルビ BLOCK    : {len(rubies)}")
    if rubies:
        rw = [int(b.get("WIDTH")) for b in rubies]
        print(f"  ルビ列の幅    : 中央値 {np.median(rw):.0f}px （以前は 11〜14px）")

    print("\n  --- 本文（先頭6行）---")
    for ln in lines[:6]:
        print(f"    [{ln.get('TYPE')}] CONF={ln.get('CONF')} {ln.get('STRING')}")

    from parseq import PARSEQ
    with open(BASE / "config" / "NDLmoji.yaml", encoding="utf-8") as f:
        charlist = list(safe_load(f)["model"]["charset_train"])
    rec = PARSEQ(model_path=str(BASE / "model" /
                 "parseq-ndl-24x768-100-tiny-153epoch-tegaki3-r8data-202604.onnx"),
                 charlist=charlist, device="cpu")
    ocr_img = cv2.imdecode(np.fromfile(str(ocr_src), dtype=np.uint8), cv2.IMREAD_COLOR)
    rubies.sort(key=lambda b: -int(b.get("X")))
    print("\n  --- ルビ列を一括認識（先頭8列）---")
    for b in rubies[:8]:
        x, y = int(b.get("X")), int(b.get("Y"))
        w, h = int(b.get("WIDTH")), int(b.get("HEIGHT"))
        pad = max(1, w // 8)
        crop = ocr_img[max(0, y - pad):min(ocr_img.shape[0], y + h + pad),
                       max(0, x - pad):min(ocr_img.shape[1], x + w + pad)]
        print(f"    幅{w:3d}px conf={b.get('CONF')} → {rec.read(crop)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
