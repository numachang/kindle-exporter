# spikes/ — 設計判断の根拠となった使い捨てスクリプト

ADR-0003 〜 ADR-0006 の実測に使ったスクリプトです。

> **これらは製品コードではありません。**
> CI の対象外で、lint も型チェックもかけていません。エラー処理も雑です。
> 残してあるのは、**ADR に書いた数字がどう出たのかを再現でき、
> `ke-cdp` を実装するときの参照になる**からです。
> 相当する機能が Rust 側に実装できたら、そのスクリプトは削除してください。

## 各スクリプトが示したこと

| スクリプト | 何を確かめたか | 対応する ADR |
|---|---|---|
| `cdp_probe.py` | DOM に本文が存在しないこと（日本語 141,987 文字中 66 文字のみ、`<ruby>` 0 件、`<canvas>` 0 件） | ADR-0004 実測 1 |
| `cdp_grab.py` | **ページ画像を原寸で取得する方法。** Amazon が `window.fetch` を差し替えているため、`Page.createIsolatedWorld` で汚染されていない JS 文脈を作り canvas 経由で抜く | ADR-0004 実測 3 |
| `measure_now.py` | 現在の表示設定での画素/文字・ルビ列幅・行 CONF の実測 | ADR-0005 実測 3 |
| `settings_sweep.py` | **localStorage を書き換えても描画に反映されない**という否定的結果（5 通りすべて同一） | ADR-0005 実測 1 |
| `font_control.py` | **`ion-range` のクリックでフォントサイズを制御できること。** 設定メニューの開閉は `ion-menu` の `show-menu` クラスで判定する（`ion-backdrop` は出ない） | ADR-0005 実測 2・3 |
| `ruby_probe.py` | ルビ列を分割せず**一括認識**すると 96% 読めること（分割すると激しく劣化する） | ADR-0003 実測 3 |
| `real_ocr.py` | 実ページでの OCR 品質（行 CONF 平均 0.927） | ADR-0004 実測 6 |
| `gpu_diag.py` | CUDA EP の可否と、DEIM 9.9 倍速・PARSeq は GPU の方が遅いこと。CPU 8 並列で 39.3 行/秒 | ADR-0003 実験 2 |

## `ke-cdp` を書くときに写すべき箇所

`cdp_grab.py` と `font_control.py` が中心です。

- **isolated world は必須。** `Page.createIsolatedWorld` を使わずに `fetch(blobURL)` を
  呼ぶと `TypeError: Failed to fetch` になる（Amazon が `window.fetch` を差し替えている）
- CDP のパラメータ名 `grantUniveralAccess` は**プロトコル側の綴り間違い**であって、
  こちらの typo ではない。直すと動かなくなる
- 設定メニューの開閉判定は `ion-menu` の `classList.contains('show-menu')`。
  `ion-backdrop` は出ないので当てにしてはいけない
- `ion-range` の `value` / `min` / `max` は JS から読めない（すべて `undefined`）。
  だから設定は冪等にできず、`ke-nav` の「設定 → 実測 → 検証」ループが必要になる
- ページ画像は `.kg-full-page-img img`。blob は same-origin なので canvas は汚染されない

## 動かし方

### 1. CDP 側（`cdp_*.py` / `font_control.py` / `measure_now.py` / `settings_sweep.py`）

Chrome を専用プロファイルで、リモートデバッグを有効にして起動する。
**Chrome 136 以降は既定プロファイルだと `--remote-debugging-port` が無視される**ため、
`--user-data-dir` の指定が必須。

```powershell
& "C:\Program Files\Google\Chrome\Application\chrome.exe" `
  --remote-debugging-port=9222 `
  --remote-allow-origins=* `
  --user-data-dir="$env:USERPROFILE\.ke-chrome-profile" `
  --no-first-run --no-default-browser-check `
  "https://read.amazon.co.jp/?asin=<ASIN>"
```

初回は Amazon へのログインが必要（以降このプロファイルに残る）。
本を開いた状態にしてから実行する。

```bash
uv venv .venv --python 3.12
uv pip install --python .venv/Scripts/python.exe websocket-client pillow numpy opencv-python-headless pyyaml
.venv/Scripts/python.exe spikes/cdp_grab.py
```

### 2. OCR 側（`ruby_probe.py` / `real_ocr.py` / `gpu_diag.py`）

NDLOCR-Lite を `spikes/ndlocr-lite/` に取得しておく（ONNX モデル 4 本、約 157MB 同梱）。

```bash
git clone --depth 1 https://github.com/ndl-lab/ndlocr-lite.git spikes/ndlocr-lite
uv pip install --python .venv/Scripts/python.exe -r spikes/ndlocr-lite/requirements.txt
```

GPU を使う場合は、`onnxruntime-gpu` に加えて **NVIDIA のランタイムを pip で入れ、
`nvidia/*/bin` を DLL 探索パスに追加する**必要がある。これを忘れると
`cudnn64_9.dll` のロードに失敗して CUDA EP が黙って使えなくなる。

```bash
uv pip install --python .venv/Scripts/python.exe \
  onnxruntime-gpu nvidia-cudnn-cu12 nvidia-cublas-cu12 nvidia-cuda-runtime-cu12 \
  nvidia-cufft-cu12 nvidia-curand-cu12
```

```python
import glob, os, sys
from pathlib import Path
for b in glob.glob(str(Path(sys.prefix) / "Lib" / "site-packages" / "nvidia" / "*" / "bin")):
    os.add_dll_directory(b)
import onnxruntime as ort   # この順序でないと CUDA EP が載らない
```
