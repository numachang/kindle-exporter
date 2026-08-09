"""スパイクC: Kindle Cloud Reader を CDP から観測する。

確認すること:
  1. DOM に本文が実文字として存在するか（存在すれば OCR 不要）
  2. ルビが <ruby>/<rt> として構造化されているか
  3. canvas 描画か、フォント難読化が使われていないか
  4. Page.captureScreenshot でページが撮影できるか（黒塗りにならないか）
  5. ページ番号など、ページ遷移を確定検出できるシグナルがあるか

前提: Chrome を --remote-debugging-port=9222 で起動し、本を開いた状態にしておく。
"""

import base64
import json
import sys
import urllib.request
from pathlib import Path

from websocket import create_connection

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 9222
OUT = Path(__file__).resolve().parent / "cdp_out"

PROBE_JS = r"""
(() => {
  const d = document;
  const txt = ((d.body && d.body.innerText) || '').replace(/\s+/g, ' ').trim();
  const rubies = [...d.querySelectorAll('ruby')];
  const canvases = [...d.querySelectorAll('canvas')].map(c => ({ w: c.width, h: c.height }));
  const faces = [];
  try { d.fonts.forEach(f => faces.push(f.family)); } catch (e) {}
  const sampleEls = [...d.querySelectorAll('*')].slice(0, 1500);
  const fams = [...new Set(sampleEls.map(e => getComputedStyle(e).fontFamily).filter(Boolean))];
  return {
    url: location.href,
    title: d.title,
    textLen: txt.length,
    textSample: txt.slice(0, 400),
    rubyCount: rubies.length,
    rubySample: rubies.slice(0, 10).map(r => {
      const rt = r.querySelector('rt');
      const rtText = rt ? rt.textContent : null;
      return { base: r.textContent.replace(rtText || '', ''), rt: rtText };
    }),
    canvasCount: canvases.length,
    canvases: canvases.slice(0, 5),
    imgCount: d.querySelectorAll('img').length,
    svgTextCount: d.querySelectorAll('svg text').length,
    fontFamilies: fams.slice(0, 8),
    fontFaces: [...new Set(faces)].slice(0, 8),
    writingMode: d.body ? getComputedStyle(d.body).writingMode : null,
    // ページ番号・位置表示の候補（遷移の確定検出に使えるか）
    pageIndicators: [...d.querySelectorAll('[class*="page"],[id*="page"],[class*="location"],[aria-label]')]
      .slice(0, 400)
      .map(e => ({ tag: e.tagName, cls: (e.className || '').toString().slice(0, 60),
                   aria: e.getAttribute('aria-label'), text: (e.textContent || '').trim().slice(0, 40) }))
      .filter(o => o.text || o.aria)
      .slice(0, 15),
  };
})()
"""


class CDP:
    def __init__(self, ws_url):
        self.ws = create_connection(ws_url, suppress_origin=True, timeout=30)
        self._id = 0

    def send(self, method, params=None, session_id=None):
        self._id += 1
        msg = {"id": self._id, "method": method, "params": params or {}}
        if session_id:
            msg["sessionId"] = session_id
        self.ws.send(json.dumps(msg))
        while True:
            resp = json.loads(self.ws.recv())
            if resp.get("id") == self._id:
                if "error" in resp:
                    raise RuntimeError(f"{method}: {resp['error']}")
                return resp.get("result", {})


def main():
    OUT.mkdir(exist_ok=True)
    version = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/version"))
    print(f"Browser : {version.get('Browser')}")
    targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list"))

    pages = [t for t in targets if t["type"] == "page"]
    print(f"\nページターゲット {len(pages)} 件:")
    for i, t in enumerate(pages):
        print(f"  [{i}] {t['title'][:60]!r}  {t['url'][:90]}")

    reader = next((t for t in pages if "read.amazon" in t["url"]), None)
    if reader is None:
        print("\n[NG] read.amazon.* のタブが見つかりません。本を開いた状態にしてください。")
        return 1
    print(f"\n対象: {reader['url']}")

    cdp = CDP(version["webSocketDebuggerUrl"])
    sid = cdp.send("Target.attachToTarget", {"targetId": reader["id"], "flatten": True})["sessionId"]
    cdp.send("Page.enable", session_id=sid)
    cdp.send("Runtime.enable", session_id=sid)
    # OOPIF（別プロセスの iframe）にも降りられるようにする
    cdp.send("Target.setAutoAttach",
             {"autoAttach": True, "waitForDebuggerOnStart": False, "flatten": True},
             session_id=sid)

    # --- フレーム構成 ---
    tree = cdp.send("Page.getFrameTree", session_id=sid)["frameTree"]
    frames = []

    def walk(node, depth=0):
        f = node["frame"]
        frames.append((depth, f["id"], f.get("url", "")))
        for c in node.get("childFrames", []):
            walk(c, depth + 1)

    walk(tree)
    print(f"\nフレーム {len(frames)} 件:")
    for depth, fid, url in frames:
        print(f"  {'  ' * depth}- {url[:100] or '(about:blank)'}")

    # --- 各フレームで DOM を観測 ---
    results = []
    for depth, fid, url in frames:
        try:
            world = cdp.send("Page.createIsolatedWorld",
                             {"frameId": fid, "worldName": "probe", "grantUniveralAccess": True},
                             session_id=sid)
            ctx = world["executionContextId"]
            r = cdp.send("Runtime.evaluate",
                         {"expression": PROBE_JS, "contextId": ctx, "returnByValue": True},
                         session_id=sid)
            val = r.get("result", {}).get("value")
            if val:
                results.append((depth, val))
        except Exception as e:  # noqa: BLE001 - 観測できないフレームは飛ばす
            print(f"  [skip] {url[:60]}: {e}")

    print("\n" + "=" * 70)
    for depth, v in results:
        if v["textLen"] == 0 and v["canvasCount"] == 0 and v["imgCount"] == 0:
            continue
        print(f"\n■ frame: {v['url'][:90]}")
        print(f"   本文テキスト長 : {v['textLen']}")
        print(f"   writing-mode   : {v['writingMode']}")
        print(f"   ruby 要素      : {v['rubyCount']}")
        if v["rubySample"]:
            for s in v["rubySample"]:
                print(f"      {s['base']!r} → {s['rt']!r}")
        print(f"   canvas / img / svg text : {v['canvasCount']} / {v['imgCount']} / {v['svgTextCount']}")
        if v["canvases"]:
            print(f"      canvas サイズ: {v['canvases']}")
        print(f"   font-family    : {v['fontFamilies']}")
        print(f"   @font-face     : {v['fontFaces']}")
        if v["textSample"]:
            print(f"   本文サンプル   : {v['textSample'][:200]}")
        if v["pageIndicators"]:
            print("   ページ表示候補 :")
            for p in v["pageIndicators"][:8]:
                print(f"      <{p['tag']}> cls={p['cls']!r} aria={p['aria']!r} text={p['text']!r}")

    # --- スクリーンショット可否 ---
    print("\n" + "=" * 70)
    shot = cdp.send("Page.captureScreenshot", {"format": "png", "captureBeyondViewport": False},
                    session_id=sid)
    png = base64.b64decode(shot["data"])
    path = OUT / "reader.png"
    path.write_bytes(png)
    print(f"スクリーンショット: {path} ({len(png):,} bytes)")
    try:
        import numpy as np
        from PIL import Image
        arr = np.asarray(Image.open(path).convert("L"), dtype=np.float32)
        print(f"  輝度 mean={arr.mean():.1f} std={arr.std():.1f} "
              f"→ {'★黒塗りの疑い' if arr.mean() < 12 or arr.std() < 3 else 'OK（内容が描画されている）'}")
    except ImportError:
        print("  （numpy/PIL がないため輝度判定はスキップ）")

    (OUT / "probe.json").write_text(
        json.dumps([v for _, v in results], ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"生データ: {OUT / 'probe.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
