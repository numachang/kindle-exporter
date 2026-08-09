"""フォントサイズと配色テーマの「現在値を読めるか」「離散的に動かせるか」を検証する。

ADR-0005 実測 2 は「`ion-range` の value / min / max は JS から読めない（すべて undefined）」
と結論し、そこから決定 2「設定は冪等にできない。設定 → 実測 → 検証のループが要る」を導いた。

しかし `settings_menu_dom.py` で ion-range に `value="5"` という**属性**が見え、
`aria-label="フォントサイズを拡大する"` の ±ボタンと 14 個の `range-tick` も見つかった。
ADR-0005 が見ていたのは JS プロパティだけだった可能性がある。

確かめること:
  1. `getAttribute('value')` は現在のフォントサイズを本当に表しているか（動かして追従するか）
  2. ±ボタンで 1 段ずつ動かせるか。段階数はいくつか
  3. 配色テーマは `#theme-White` などのクリックで変えられ、`aria-checked` で観測できるか
"""

import json
import sys
import time
import urllib.request

from websocket import create_connection

PORT = 9222

MENU_OPEN = "(() => { const m = document.querySelector('ion-menu'); " \
            "return !!m && m.classList.contains('show-menu'); })()"
MENU_TOGGLE = "(() => { const b = [...document.querySelectorAll('ion-button,button')]" \
              ".find(e => e.getAttribute('aria-label') === 'リーダー設定'); " \
              "if (b) { b.click(); return true; } return false; })()"

# 表示設定の現在値を、読めるものすべて読む。
STATE = r"""
(() => {
  const range = document.querySelector('ion-range.font-size-slider')
             || [...document.querySelectorAll('ion-range')]
                  .find(e => e.getBoundingClientRect().width > 100);
  if (!range) return JSON.stringify({ error: 'ion-range が見つかりません' });
  const sr = range.shadowRoot;
  const ticks = sr ? [...sr.querySelectorAll('.range-tick')] : [];
  const themes = [...document.querySelectorAll('.theme-selector [role="radio"]')]
    .map(e => ({ id: e.id, value: e.getAttribute('value'),
                 checked: e.getAttribute('aria-checked') }));
  const margins = [...document.querySelectorAll('.margin-selector [role="radio"]')]
    .map(e => ({ id: e.id, checked: e.getAttribute('aria-checked') }));
  const img = document.querySelector('.kg-full-page-img img');
  return JSON.stringify({
    prop: range.value ?? null,                       // ADR-0005 が見ていたもの
    attr: range.getAttribute('value'),               // 属性としての現在値
    min: range.getAttribute('min'), max: range.getAttribute('max'),
    ticks: ticks.length,
    ticksActive: ticks.filter(t => t.classList.contains('range-tick-active')).length,
    themes, margins,
    imgNatural: img ? [img.naturalWidth, img.naturalHeight] : null,
    imgSrc: img ? (img.currentSrc || img.src || '').slice(-12) : null,
  });
})()
"""


def rect_of(selector):
    return f"""
(() => {{
  const e = document.querySelector({selector!r});
  if (!e) return null;
  const r = e.getBoundingClientRect();
  return JSON.stringify([r.x + r.width / 2, r.y + r.height / 2]);
}})()
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

    def ev(self, expr):
        self._n += 1
        fid = self.cdp.send("Page.getFrameTree", session_id=self.sid)["frameTree"]["frame"]["id"]
        ctx = self.cdp.send("Page.createIsolatedWorld",
                            {"frameId": fid, "worldName": f"rb{self._n}",
                             "grantUniveralAccess": True},
                            session_id=self.sid)["executionContextId"]
        r = self.cdp.send("Runtime.evaluate",
                          {"expression": expr, "contextId": ctx, "returnByValue": True},
                          session_id=self.sid)
        if "exceptionDetails" in r:
            raise RuntimeError(json.dumps(r["exceptionDetails"], ensure_ascii=False)[:250])
        return r["result"].get("value")

    def click_at(self, x, y):
        for t in ("mousePressed", "mouseReleased"):
            self.cdp.send("Input.dispatchMouseEvent",
                          {"type": t, "x": x, "y": y, "button": "left", "clickCount": 1},
                          session_id=self.sid)

    def click(self, selector):
        raw = self.ev(rect_of(selector))
        if not raw:
            print(f"    {selector} が見つかりません")
            return False
        x, y = json.loads(raw)
        self.click_at(x, y)
        return True

    def state(self):
        return json.loads(self.ev(STATE))


def set_menu(rd, want_open):
    for _ in range(3):
        if bool(rd.ev(MENU_OPEN)) == want_open:
            return True
        rd.ev(MENU_TOGGLE)
        time.sleep(2.0)
    return bool(rd.ev(MENU_OPEN)) == want_open


def show(tag, s):
    print(f"  {tag:<22} attr={s['attr']!r} prop={s['prop']!r} "
          f"tick={s['ticksActive']}/{s['ticks']} 画像={s['imgNatural']} src…{s['imgSrc']}")


BIGGER = 'span[aria-label="フォントサイズを拡大する"]'
SMALLER = 'span[aria-label="フォントサイズを縮小する"]'


def probe_font(rd):
    print("=" * 78)
    print("1. フォントサイズの現在値は読めるか / ±ボタンで動かせるか")
    print("=" * 78)
    s = rd.state()
    if s.get("error"):
        print(s["error"])
        return None
    print(f"  min={s['min']!r} max={s['max']!r} 段階数={s['ticks']}")
    show("初期状態", s)

    for i in range(3):
        rd.click(BIGGER)
        time.sleep(2.5)
        show(f"拡大 x{i + 1}", rd.state())
    for i in range(3):
        rd.click(SMALLER)
        time.sleep(2.5)
        show(f"縮小 x{i + 1}", rd.state())
    return s


def probe_floor(rd, ticks):
    """最小まで下げてから既知の段数だけ上げ、値が一致するかを見る。"""
    print()
    print("=" * 78)
    print("2. 最小まで下げてから N 段上げると value == N になるか")
    print("=" * 78)
    for _ in range(ticks + 2):
        rd.click(SMALLER)
        time.sleep(0.5)
    time.sleep(2.0)
    show("最小まで下げた", rd.state())
    for i in range(4):
        rd.click(BIGGER)
        time.sleep(1.2)
    time.sleep(2.0)
    show("そこから 4 段上げた", rd.state())


def probe_theme(rd):
    print()
    print("=" * 78)
    print("3. 配色テーマはクリックで変えられ、aria-checked で観測できるか")
    print("=" * 78)
    for want in ("Dark", "White"):
        rd.click(f"#theme-{want}")
        time.sleep(2.5)
        s = rd.state()
        checked = [t["value"] for t in s["themes"] if t["checked"] == "true"]
        print(f"  #theme-{want} をクリック → aria-checked={checked} 画像={s['imgNatural']}")
    print(f"  マージン: {rd.state()['margins']}")


def main():
    rd = Reader()
    if not set_menu(rd, True):
        print("設定メニューを開けませんでした（本が開いていない？）")
        return 1
    s = probe_font(rd)
    if s is None:
        return 1
    probe_floor(rd, s["ticks"])
    probe_theme(rd)
    set_menu(rd, False)
    return 0


if __name__ == "__main__":
    sys.exit(main())
