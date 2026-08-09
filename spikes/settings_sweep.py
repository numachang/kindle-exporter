"""KWR_Display_Settings を振って、OCR にとって最適な表示設定を探す。

fontSizeIndex と maxNumberColumns を変えながら、
画素/文字・ルビ列幅・行CONF・1ページあたり文字数を実測する。

元の設定は最初に退避し、最後に必ず復元する。
"""

import base64
import json
import subprocess
import sys
import time
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

import cv2
import numpy as np
from websocket import create_connection

SP = Path(__file__).resolve().parent
BASE = SP / "ndlocr-lite" / "src"
OUT = SP / "cdp_out" / "settings"
PORT = 9222
KEY = "KWR_Display_Settings"

GRAB = r"""
(async () => {
  const img = document.querySelector('.kg-full-page-img img') || document.querySelector('#kr-renderer img');
  if (!img) return JSON.stringify({ error: 'no img' });
  if (!img.complete || !img.naturalWidth) await new Promise(r => { img.onload = r; setTimeout(r, 6000); });
  const c = document.createElement('canvas');
  c.width = img.naturalWidth; c.height = img.naturalHeight;
  c.getContext('2d').drawImage(img, 0, 0);
  return JSON.stringify({ natural: [img.naturalWidth, img.naturalHeight],
                          viewport: [innerWidth, innerHeight],
                          dataUrl: c.toDataURL('image/png') });
})()
"""


class CDP:
    def __init__(self, url):
        self.ws = create_connection(url, suppress_origin=True, timeout=120)
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


def ctx_of(cdp, sid, name):
    fid = cdp.send("Page.getFrameTree", session_id=sid)["frameTree"]["frame"]["id"]
    return cdp.send("Page.createIsolatedWorld",
                    {"frameId": fid, "worldName": name, "grantUniveralAccess": True},
                    session_id=sid)["executionContextId"]


def evaluate(cdp, sid, expr, name="sw", await_promise=False):
    ctx = ctx_of(cdp, sid, name)
    r = cdp.send("Runtime.evaluate",
                 {"expression": expr, "contextId": ctx, "returnByValue": True,
                  "awaitPromise": await_promise}, session_id=sid)
    if "exceptionDetails" in r:
        raise RuntimeError(json.dumps(r["exceptionDetails"], ensure_ascii=False)[:200])
    return r["result"].get("value")


def measure(cdp, sid, tag):
    raw = evaluate(cdp, sid, GRAB, f"g{tag}", await_promise=True)
    if not isinstance(raw, str):
        return None
    v = json.loads(raw)
    if v.get("error"):
        return None
    png = base64.b64decode(v["dataUrl"].split(",", 1)[1])
    src = OUT / f"{tag}.png"
    src.write_bytes(png)
    img = cv2.imdecode(np.frombuffer(png, np.uint8), cv2.IMREAD_COLOR)
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
    if gray.mean() < 110:
        src = OUT / f"{tag}_inv.png"
        cv2.imencode(".png", 255 - img)[1].tofile(str(src))

    od = OUT / f"ocr_{tag}"
    od.mkdir(parents=True, exist_ok=True)
    subprocess.run([sys.executable, "src/ocr.py", "--sourceimg", str(src), "--output", str(od)],
                   cwd=str(BASE.parent), check=True, capture_output=True)
    root = ET.parse(od / f"{src.stem}.xml").getroot()
    lines = list(root.iter("LINE"))
    rub = [b for b in root.iter("BLOCK") if b.get("TYPE") == "ルビ"]
    confs = [float(ln.get("CONF", 0)) for ln in lines]
    widths = [int(ln.get("WIDTH")) for ln in lines if int(ln.get("HEIGHT")) > int(ln.get("WIDTH"))]
    return {
        "natural": v["natural"],
        "lines": len(lines),
        "chars": sum(len(ln.get("STRING") or "") for ln in lines),
        "conf": float(np.mean(confs)) if confs else 0.0,
        "px": float(np.median(widths)) if widths else 0.0,
        "ruby": len(rub),
        "ruby_px": float(np.median([int(b.get("WIDTH")) for b in rub])) if rub else 0.0,
        "sample": (lines[1].get("STRING") if len(lines) > 1 else ""),
    }


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    ver = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/version"))
    tgts = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list"))
    page = next(t for t in tgts if t["type"] == "page" and "read.amazon" in t["url"])
    cdp = CDP(ver["webSocketDebuggerUrl"])
    sid = cdp.send("Target.attachToTarget", {"targetId": page["id"], "flatten": True})["sessionId"]
    cdp.send("Runtime.enable", session_id=sid)
    cdp.send("Page.enable", session_id=sid)

    original = evaluate(cdp, sid, f"localStorage.getItem({KEY!r})", "orig")
    (OUT / "original_settings.json").write_text(original or "", encoding="utf-8")
    print(f"元の設定を退避しました → {OUT / 'original_settings.json'}")
    base_cfg = json.loads(original)
    print(f"  fontSizeIndex={base_cfg.get('fontSizeIndex')} fontSize={base_cfg.get('fontSize')} "
          f"columns={base_cfg.get('maxNumberColumns')} theme={base_cfg.get('theme')} "
          f"margins={base_cfg.get('sideMarginsSize')}")

    configs = [
        ("idx5_col2", {"fontSizeIndex": 5, "maxNumberColumns": 2}),
        ("idx5_col1", {"fontSizeIndex": 5, "maxNumberColumns": 1}),
        ("idx8_col1", {"fontSizeIndex": 8, "maxNumberColumns": 1}),
        ("idx11_col1", {"fontSizeIndex": 11, "maxNumberColumns": 1}),
        ("idx14_col1", {"fontSizeIndex": 14, "maxNumberColumns": 1}),
    ]

    rows = []
    try:
        for tag, over in configs:
            cfg = {**base_cfg, **over, "theme": 1, "sideMarginsSize": "narrow"}
            evaluate(cdp, sid,
                     f"localStorage.setItem({KEY!r}, {json.dumps(json.dumps(cfg))}); 1",
                     f"set{tag}")
            cdp.send("Page.reload", session_id=sid)
            time.sleep(12)
            try:
                m = measure(cdp, sid, tag)
            except Exception as e:  # noqa: BLE001
                print(f"  [{tag}] 計測失敗: {str(e)[:120]}")
                m = None
            if m:
                m["tag"] = tag
                m["idx"] = over["fontSizeIndex"]
                m["col"] = over["maxNumberColumns"]
                rows.append(m)
                print(f"  [{tag}] 計測完了")
    finally:
        if original:
            evaluate(cdp, sid, f"localStorage.setItem({KEY!r}, {json.dumps(original)}); 1", "restore")
            cdp.send("Page.reload", session_id=sid)
            print("\n★ 元の表示設定に復元し、リロードしました")

    print("\n" + "=" * 92)
    print(f"{'設定':<12} {'画像原寸':>12} {'行':>4} {'文字/頁':>7} {'行CONF':>7} "
          f"{'画素/文字':>9} {'ルビ数':>6} {'ルビ幅':>6}")
    print("=" * 92)
    for r in rows:
        print(f"idx{r['idx']:<2} col{r['col']:<4} {r['natural'][0]:>5}x{r['natural'][1]:<6} "
              f"{r['lines']:>4} {r['chars']:>7} {r['conf']:>7.3f} {r['px']:>9.0f} "
              f"{r['ruby']:>6} {r['ruby_px']:>6.0f}")
    print("\n本文サンプル:")
    for r in rows:
        print(f"  idx{r['idx']} col{r['col']}: {r['sample'][:70]}")
    (OUT / "sweep.json").write_text(json.dumps(rows, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
