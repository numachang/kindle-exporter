"""フォントサイズの ion-range を CDP から操作できるか検証する。

capture フェーズが書籍ごとに最適なフォントサイズを自動設定できるかどうかを決める。
操作できるなら、それが OCR 精度の主要レバーになる。
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
OUT = SP / "cdp_out" / "font"
PORT = 9222

RANGE_INFO = r"""
(() => {
  const r = [...document.querySelectorAll('ion-range')]
    .find(e => !e.getAttribute('aria-label') && e.getBoundingClientRect().width > 100);
  if (!r) return JSON.stringify({ error: 'font ion-range が見つかりません（メニューが閉じている？）' });
  const b = r.getBoundingClientRect();
  return JSON.stringify({
    value: r.value ?? null,
    rect: [Math.round(b.x), Math.round(b.y), Math.round(b.width), Math.round(b.height)],
  });
})()
"""

# 設定メニューは ion-menu の show-menu クラスで開閉を判定する（backdrop は出ない）
MENU_OPEN = "(() => { const m = document.querySelector('ion-menu'); " \
            "return !!m && m.classList.contains('show-menu'); })()"
MENU_TOGGLE = "(() => { const b = [...document.querySelectorAll('ion-button,button')]" \
              ".find(e => e.getAttribute('aria-label') === 'リーダー設定'); " \
              "if (b) { b.click(); return true; } return false; })()"

GRAB = r"""
(async () => {
  const img = document.querySelector('.kg-full-page-img img') || document.querySelector('#kr-renderer img');
  if (!img) return JSON.stringify({ error: 'no img' });
  if (!img.complete || !img.naturalWidth) await new Promise(r => { img.onload = r; setTimeout(r, 6000); });
  const c = document.createElement('canvas');
  c.width = img.naturalWidth; c.height = img.naturalHeight;
  c.getContext('2d').drawImage(img, 0, 0);
  return JSON.stringify({ natural: [img.naturalWidth, img.naturalHeight],
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


class Reader:
    def __init__(self):
        ver = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/version"))
        tgts = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list"))
        page = next(t for t in tgts if t["type"] == "page" and "read.amazon" in t["url"])
        self.cdp = CDP(ver["webSocketDebuggerUrl"])
        self.sid = self.cdp.send("Target.attachToTarget",
                                 {"targetId": page["id"], "flatten": True})["sessionId"]
        self.cdp.send("Runtime.enable", session_id=self.sid)
        self.cdp.send("Page.enable", session_id=self.sid)
        self._n = 0

    def ev(self, expr, await_promise=False):
        self._n += 1
        fid = self.cdp.send("Page.getFrameTree", session_id=self.sid)["frameTree"]["frame"]["id"]
        ctx = self.cdp.send("Page.createIsolatedWorld",
                            {"frameId": fid, "worldName": f"w{self._n}", "grantUniveralAccess": True},
                            session_id=self.sid)["executionContextId"]
        r = self.cdp.send("Runtime.evaluate",
                          {"expression": expr, "contextId": ctx, "returnByValue": True,
                           "awaitPromise": await_promise}, session_id=self.sid)
        if "exceptionDetails" in r:
            raise RuntimeError(json.dumps(r["exceptionDetails"], ensure_ascii=False)[:250])
        return r["result"].get("value")

    def click(self, x, y):
        for t in ("mousePressed", "mouseReleased"):
            self.cdp.send("Input.dispatchMouseEvent",
                          {"type": t, "x": x, "y": y, "button": "left", "clickCount": 1},
                          session_id=self.sid)

    def key(self, k, code, vk):
        for t in ("keyDown", "keyUp"):
            self.cdp.send("Input.dispatchKeyEvent",
                          {"type": t, "key": k, "code": code, "windowsVirtualKeyCode": vk,
                           "nativeVirtualKeyCode": vk}, session_id=self.sid)


def measure(rd, tag):
    raw = rd.ev(GRAB, await_promise=True)
    if not isinstance(raw, str):
        return None
    v = json.loads(raw)
    if v.get("error"):
        return None
    png = base64.b64decode(v["dataUrl"].split(",", 1)[1])
    OUT.mkdir(parents=True, exist_ok=True)
    src = OUT / f"{tag}.png"
    src.write_bytes(png)
    img = cv2.imdecode(np.frombuffer(png, np.uint8), cv2.IMREAD_COLOR)
    if cv2.cvtColor(img, cv2.COLOR_BGR2GRAY).mean() < 110:
        src = OUT / f"{tag}_inv.png"
        cv2.imencode(".png", 255 - img)[1].tofile(str(src))
    od = OUT / f"ocr_{tag}"
    od.mkdir(exist_ok=True)
    subprocess.run([sys.executable, "src/ocr.py", "--sourceimg", str(src), "--output", str(od)],
                   cwd=str(BASE.parent), check=True, capture_output=True)
    root = ET.parse(od / f"{src.stem}.xml").getroot()
    lines = list(root.iter("LINE"))
    rub = [b for b in root.iter("BLOCK") if b.get("TYPE") == "ルビ"]
    confs = [float(ln.get("CONF", 0)) for ln in lines]
    widths = [int(ln.get("WIDTH")) for ln in lines if int(ln.get("HEIGHT")) > int(ln.get("WIDTH"))]
    return {"chars": sum(len(ln.get("STRING") or "") for ln in lines),
            "lines": len(lines),
            "conf": float(np.mean(confs)) if confs else 0.0,
            "px": float(np.median(widths)) if widths else 0.0,
            "ruby": len(rub),
            "ruby_px": float(np.median([int(b.get("WIDTH")) for b in rub])) if rub else 0.0}


def set_menu(rd, want_open):
    """設定メニューを開閉する。"""
    for _ in range(3):
        if bool(rd.ev(MENU_OPEN)) == want_open:
            return True
        rd.ev(MENU_TOGGLE)
        time.sleep(2.5)
    return bool(rd.ev(MENU_OPEN)) == want_open


def main():
    rd = Reader()
    if not set_menu(rd, True):
        print("設定メニューを開けませんでした")
        return 1
    info = json.loads(rd.ev(RANGE_INFO))
    if info.get("error"):
        print(info["error"])
        return 1
    print(f"フォントサイズ ion-range: rect={info['rect']} value={info['value']}")

    x0, y0, w, h = info["rect"]
    cy = y0 + h // 2
    results = []

    # スライダーの各位置をクリックして、フォントサイズ段階ごとに実測する
    for i, frac in enumerate([0.05, 0.35, 0.65, 0.95]):
        if not set_menu(rd, True):
            break
        cx = int(x0 + w * frac)
        rd.click(cx, cy)
        time.sleep(3.0)
        set_menu(rd, False)
        time.sleep(4.0)
        m = measure(rd, f"f{i}")
        if m:
            m["frac"] = frac
            results.append(m)
            print(f"  スライダー {frac:.0%} → 画素/文字={m['px']:.0f}  "
                  f"文字/頁={m['chars']}  CONF={m['conf']:.3f}  ルビ幅={m['ruby_px']:.0f}")
        else:
            print(f"  スライダー {frac:.0%} → 計測失敗")

    set_menu(rd, False)

    print("\n" + "=" * 80)
    print(f"{'range':>7} {'画素/文字':>9} {'ルビ幅':>7} {'文字/頁':>8} {'行':>4} {'行CONF':>8} {'ルビ数':>6}")
    print("=" * 80)
    for r in results:
        print(f"{r['frac']:>7.0%} {r['px']:>9.0f} {r['ruby_px']:>7.0f} "
              f"{r['chars']:>8} {r['lines']:>4} {r['conf']:>8.3f} {r['ruby']:>6}")
    (OUT / "font_sweep.json").write_text(json.dumps(results, ensure_ascii=False, indent=2),
                                         encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())

