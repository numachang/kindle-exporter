"""スパイクC-4: ページ画像（blob）を直接取得し、OCR 入力としての素性を調べる。

Amazon が window.fetch を差し替えているため、素の fetch は失敗する。
Page.createIsolatedWorld で組み込みが汚染されていない JS 文脈を作り、
そこから canvas 経由で原寸ビットマップを取り出す。

確認すること:
  1. ページ画像を原寸で取り出せるか
  2. 画像そのものが白地黒字か、黒地白字か（CSS 反転かどうか）
  3. viewport を拡大すると、より高解像度のページ画像が配信されるか
"""

import base64
import json
import sys
import urllib.request
from pathlib import Path

from websocket import create_connection

PORT = 9222
OUT = Path(__file__).resolve().parent / "cdp_out"

GRAB = r"""
(async () => {
  const img = document.querySelector('.kg-full-page-img img')
           || document.querySelector('#kr-renderer img');
  if (!img) return JSON.stringify({ error: 'ページ画像が見つかりません' });
  if (!img.complete || !img.naturalWidth) {
    await new Promise(r => { img.onload = r; setTimeout(r, 3000); });
  }
  const c = document.createElement('canvas');
  c.width = img.naturalWidth;
  c.height = img.naturalHeight;
  c.getContext('2d').drawImage(img, 0, 0);
  let dataUrl = null, err = null;
  try { dataUrl = c.toDataURL('image/png'); } catch (e) { err = String(e); }
  const r = img.getBoundingClientRect();
  return JSON.stringify({
    src: (img.currentSrc || img.src || '').slice(0, 120),
    natural: [img.naturalWidth, img.naturalHeight],
    shown: [Math.round(r.width), Math.round(r.height)],
    filter: getComputedStyle(img).filter,
    parentFilter: getComputedStyle(img.parentElement).filter,
    dpr: devicePixelRatio,
    viewport: [innerWidth, innerHeight],
    taintError: err,
    dataUrl,
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


def isolated_ctx(cdp, sid):
    """本文フレームに、組み込みが汚染されていない JS 文脈を作る。"""
    fid = cdp.send("Page.getFrameTree", session_id=sid)["frameTree"]["frame"]["id"]
    w = cdp.send("Page.createIsolatedWorld",
                 {"frameId": fid, "worldName": "grab", "grantUniveralAccess": True},
                 session_id=sid)
    return w["executionContextId"]


def grab(cdp, sid, label):
    ctx = isolated_ctx(cdp, sid)
    r = cdp.send("Runtime.evaluate",
                 {"expression": GRAB, "contextId": ctx,
                  "returnByValue": True, "awaitPromise": True},
                 session_id=sid)
    if "exceptionDetails" in r:
        print(f"  [{label}] JS 例外: {json.dumps(r['exceptionDetails'], ensure_ascii=False)[:300]}")
        return None
    raw = r["result"].get("value")
    if not isinstance(raw, str):
        print(f"  [{label}] 想定外の戻り値: {json.dumps(r['result'], ensure_ascii=False)[:300]}")
        return None
    v = json.loads(raw)
    if v.get("error"):
        print(f"  [{label}] {v['error']}")
        return None

    print(f"  [{label}] viewport={v['viewport']} dpr={v['dpr']}")
    print(f"        原寸={v['natural']} 表示={v['shown']}")
    print(f"        CSS filter: img={v['filter']}  parent={v['parentFilter']}")
    if v.get("taintError"):
        print(f"        canvas 汚染で取り出し不可: {v['taintError']}")
        return None
    data = base64.b64decode(v["dataUrl"].split(",", 1)[1])
    path = OUT / f"page_{label}.png"
    path.write_bytes(data)
    print(f"        → {path.name} ({len(data):,} bytes)")
    return path


def analyze(path):
    import numpy as np
    from PIL import Image
    a = np.asarray(Image.open(path).convert("L"), dtype=np.uint8)
    print(f"        輝度 mean={a.mean():.1f} 暗={((a < 96).mean() * 100):.1f}% "
          f"明={((a > 160).mean() * 100):.1f}% "
          f"→ {'黒地白字（OCR前に反転が必要）' if a.mean() < 110 else '白地黒字（そのままOCR可）'}")


def main():
    OUT.mkdir(exist_ok=True)
    ver = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/version"))
    tgts = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list"))
    page = next(t for t in tgts if t["type"] == "page" and "read.amazon" in t["url"])
    cdp = CDP(ver["webSocketDebuggerUrl"])
    sid = cdp.send("Target.attachToTarget", {"targetId": page["id"], "flatten": True})["sessionId"]
    cdp.send("Runtime.enable", session_id=sid)
    cdp.send("Page.enable", session_id=sid)

    print("=" * 70)
    print("1. 現在の viewport でページ画像を原寸取得")
    print("=" * 70)
    p = grab(cdp, sid, "native")
    if p:
        analyze(p)

    print("\n" + "=" * 70)
    print("2. viewport を拡大すると高解像度の画像が配信されるか")
    print("=" * 70)
    for w, h, label in [(2000, 2400, "w2000"), (3200, 3800, "w3200")]:
        cdp.send("Emulation.setDeviceMetricsOverride",
                 {"width": w, "height": h, "deviceScaleFactor": 1, "mobile": False},
                 session_id=sid)
        cdp.send("Runtime.evaluate",
                 {"expression": "new Promise(r=>setTimeout(r,4000))", "awaitPromise": True},
                 session_id=sid)
        p2 = grab(cdp, sid, label)
        if p2:
            analyze(p2)
    cdp.send("Emulation.clearDeviceMetricsOverride", session_id=sid)
    print("\n（viewport の上書きは解除しました）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
