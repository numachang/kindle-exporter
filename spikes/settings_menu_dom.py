"""設定メニューの DOM を掘り、テーマ切り替えの操作子を特定する。

ROADMAP「残っている不確実性 #2」への回答を得るためのスクリプト。
`Action::SetTheme` に対応する UI 操作が未特定なので、`aria-label="リーダー設定"` を
押して開いた `ion-menu` の中身を、shadow root を貫通して列挙する。

Ionic のコンポーネントは shadow DOM を多用するため、`querySelectorAll` だけでは
中身が見えない。`shadowRoot` を再帰的にたどる必要がある。
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

# ion-menu 配下を shadow root ごと歩き、操作子になりうる要素を列挙する。
DUMP = r"""
(() => {
  const menu = document.querySelector('ion-menu');
  if (!menu) return JSON.stringify({ error: 'ion-menu が見つかりません' });

  const out = [];
  const walk = (node, depth, shadow) => {
    if (depth > 24) return;
    for (const el of node.children || []) {
      const r = el.getBoundingClientRect();
      const own = [...el.childNodes]
        .filter(n => n.nodeType === 3).map(n => n.textContent.trim()).join(' ').trim();
      out.push({
        d: depth,
        s: shadow,
        tag: el.tagName.toLowerCase(),
        id: el.id || null,
        cls: (el.getAttribute('class') || '').slice(0, 90) || null,
        aria: el.getAttribute('aria-label'),
        role: el.getAttribute('role'),
        val: el.getAttribute('value'),
        checked: el.getAttribute('aria-checked') ?? el.getAttribute('checked'),
        text: own.slice(0, 40) || null,
        rect: r.width ? [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)] : null,
        bg: getComputedStyle(el).backgroundColor,
      });
      if (el.shadowRoot) walk(el.shadowRoot, depth + 1, true);
      walk(el, depth + 1, shadow);
    }
  };
  walk(menu, 0, false);
  return JSON.stringify({ count: out.length, nodes: out });
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
        """isolated world で評価する（Amazon が組み込みを差し替えているため必須）。"""
        self._n += 1
        fid = self.cdp.send("Page.getFrameTree", session_id=self.sid)["frameTree"]["frame"]["id"]
        ctx = self.cdp.send("Page.createIsolatedWorld",
                            {"frameId": fid, "worldName": f"dump{self._n}",
                             "grantUniveralAccess": True},  # 綴り間違いはプロトコル側の仕様
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


def set_menu(rd, want_open):
    for _ in range(3):
        if bool(rd.ev(MENU_OPEN)) == want_open:
            return True
        rd.ev(MENU_TOGGLE)
        time.sleep(2.5)
    return bool(rd.ev(MENU_OPEN)) == want_open


def main():
    rd = Reader()
    if not set_menu(rd, True):
        print("設定メニューを開けませんでした（本が開いていない？）")
        return 1
    v = json.loads(rd.ev(DUMP))
    if v.get("error"):
        print(v["error"])
        return 1

    nodes = v["nodes"]
    print(f"ion-menu 配下: {v['count']} ノード\n")
    for n in nodes:
        # 画面に出ていない・情報の無いノードは省く
        if not n["rect"] and not n["aria"] and not n["text"]:
            continue
        pad = "  " * n["d"]
        mark = "#" if n["s"] else " "
        bits = [f"{pad}{mark}<{n['tag']}>"]
        for k in ("id", "aria", "role", "val", "checked", "text"):
            if n[k]:
                bits.append(f"{k}={n[k]!r}")
        if n["cls"]:
            bits.append(f"class={n['cls']!r}")
        if n["rect"]:
            bits.append(f"rect={n['rect']}")
        if n["bg"] not in ("rgba(0, 0, 0, 0)", "transparent"):
            bits.append(f"bg={n['bg']}")
        print(" ".join(bits))
    return 0


if __name__ == "__main__":
    sys.exit(main())
