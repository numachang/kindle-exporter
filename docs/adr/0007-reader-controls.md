# ADR-0007: リーダー操作子の特定と、表示設定の冪等化

- **ステータス:** 採択
- **日付:** 2026-08-09
- **影響:** [ADR-0005](0005-reader-display-settings.md) の実測 2 と決定 2 を**置き換える**。
  [ADR-0004](0004-page-acquisition.md) 決定 3 のページ遷移方式を具体化する。

## 背景

[ROADMAP](../ROADMAP.md) の残課題「テーマを白にする UI 操作が未特定」を解くため、
設定メニューの DOM を shadow root ごと掘った。目的は達したが、
**ついでに [ADR-0005](0005-reader-display-settings.md) の前提が 1 つ覆った。**

対象: 日本語の縦書き小説（ルビ多数・ページ番号を持たない）、
Kindle Cloud Reader、Chrome 151、viewport 1832x1278。
再現手順は [spikes/settings_menu_dom.py](../../spikes/settings_menu_dom.py) と
[spikes/font_readback.py](../../spikes/font_readback.py)。

## 実測

### 1. テーマの操作子は `#theme-White` などの radio

設定メニュー内に `role="radiogroup"` の `.theme-selector` があり、
その中に 4 つの radio が入っている。

| id | `value` | `aria-label` |
|---|---|---|
| `theme-White` | `White` | 白 |
| `theme-Dark` | `Dark` | 濃い |
| `theme-Sepia` | `Sepia` | セピア |
| `theme-Green` | `Green` | 緑 |

**クリックで切り替わり、`aria-checked` で現在値を観測できる。**
テーマは白・暗の 2 値ではなく 4 値である。

### 2. フォントサイズの現在値は「属性」としてなら読める

[ADR-0005](0005-reader-display-settings.md) 実測 2 は
「`ion-range` の `value` / `min` / `max` は JS から読めない（すべて `undefined`）」
と記録し、そこから決定 2「設定は冪等にできない」を導いていた。

**これは JS プロパティだけを見た結論だった。属性は読める。**

```
ion-range.font-size-slider  getAttribute('value')="5"  min="0"  max="13"
range.value（プロパティ）    null                    ← ADR-0005 が見ていたもの
shadowRoot の .range-tick   14 個（うち .range-tick-active が 6 個）
```

さらに、スライダーの左右に**専用の増減ボタン**がある。

```html
<span role="button" aria-label="フォントサイズを縮小する">A</span>
<span role="button" aria-label="フォントサイズを拡大する">A</span>
```

押すと 1 段ずつ、完全に決定的に動く。

| 操作 | `value` | active tick |
|---|---|---|
| 初期 | 5 | 6/14 |
| 拡大 ×3 | 6 → 7 → 8 | 7 → 8 → 9 |
| 縮小 ×3 | 7 → 6 → 5 | 8 → 7 → 6 |
| 最小まで縮小 | **0** | 1/14 |
| そこから拡大 ×4 | **4** | 5/14 |

**フォントサイズは 0〜13 の 14 段で、現在値が読め、目標値へ確実に到達できる。**
すなわち**設定は冪等にできる。**

### 3. `.click()` は効かない。`Input.dispatchMouseEvent` は効く

ページ送りボタンに対する 2 通りの操作を比較した。

| 操作 | 結果 |
|---|---|
| isolated world から `button.click()` | **動かない** |
| `Input.dispatchMouseEvent`（press + release） | 動く |

一方、設定メニューのトグル（`ion-button`）は `.click()` でも
`Input.dispatchMouseEvent` でも動く。**要素によって効く機構が違う。**
両方に効く `Input.dispatchMouseEvent` に統一すれば、この差を意識せずに済む。

### 4. ページ送りは chevron ボタン。矢印キーは使えない

```
#kr-chevron-left   aria-label="次のページ"   中心 (92, 639)
#kr-chevron-right  aria-label="前のページ"   中心 (1741, 639)
```

**縦書き（右→左に読む）書籍では、左の chevron が「次」である。**

矢印キーは使えない。`Input.dispatchKeyEvent` の ArrowRight は
**フォーカスされている `#kr-scrubber-bar`（位置シークバー）を 1 目盛り動かすだけ**で、
ページ送りにはならなかった。位置が 9783 → 9782 と 1 だけ動いたのがその証拠である
（1 ページは約 41〜55 位置ぶん）。Space キーはページ送りとして機能したが、
「前へ」に対応するキーが無いので採用しない。

**読み進む向きは CSS からは判定できない。** 本文がサーバ側でレンダリングされた
画像である（[ADR-0004](0004-page-acquisition.md)）ため、`#kr-renderer` の
`writing-mode` は `horizontal-tb`、`direction` は `ltr` を返す。
`<html>` の `dir` / `lang` 属性も無い。**向きを知る手段は `aria-label` だけである。**

### 5. 位置表示には「ページ」形式と「位置」形式がある

[ADR-0004](0004-page-acquisition.md) 実測 4 では `33/431ページ` を観測していたが、
本 ADR の対象書籍は**ページ番号を持たず**、`位置9783/10167 ● 96%` と表示する
（`.text-div`）。設定メニューの「本のページ」チェックボックス
（`#page-in-book-item`）は on になっているので、**書籍側にページ番号が無い**。

1 ページあたり 41〜55 位置ぶん進む。したがって**最終ページの「位置」が
総数と一致するとは限らず、「位置」形式の書籍では巻末を確定できない。**

### 6. 所要時間（本設計で最も知りたかった数字）

`kindle_shot` は 8.13 秒/ページだった（[ADR-0001](0001-architecture.md)）。

送り → 確定待ち → 画像取得 のループを 25 ページ × 3 条件で実測した。
確定待ちは「位置表示が変わる、または `<img>` の blob URL が変わって
`complete` になる」で判定し、1 秒待って変化が無ければ再クリックする。

| クリック前の待ち | 中央値 | 平均 | 最大 | クリック回数 |
|---|---|---|---|---|
| **0ms** | **124ms/頁** | 265ms/頁 | 1143ms | 27 / 25 頁 |
| 120ms | 246ms/頁 | 313ms/頁 | 1212ms | 25 / 25 頁 |
| 250ms | 384ms/頁 | 425ms/頁 | 1210ms | 25 / 25 頁 |

**待たずに送り、空振りしたら再クリックするのが最も速い。**
連続クリックは 25 回に 2 回ほど空振りするが、再クリックの費用の方が
一律に待つ費用より安い。

内訳と周辺コスト:

| 項目 | 実測値 |
|---|---|
| ページ画像の取り出し（canvas → PNG dataURL） | **17ms**（2199x1692、約 600KB） |
| `Page.getFrameTree` + `Page.createIsolatedWorld` | 1.2ms |
| 既存 isolated world での `Runtime.evaluate` | 0.3ms |
| 観測 1 回（位置・画像・メニュー・テーマをまとめて 1 回で取る） | 0.4ms |

**CDP の呼び出しコストは無視できる。** isolated world を毎回作り直しても
1.2ms なので、性能のために使い回す必要はない。

### 7. シークバーの `value` は現在位置を表さない

`#kr-scrubber-bar` は `min="1" max="10167"` を持つが、`value` 属性は
位置 9783 のとき 384、位置 9887 のとき 280 と、**現在位置と無関係な値**を返した。
`aria-label` にだけ現在位置が入る（`aria-label="位置9783"`）。
**巻き戻しの近道には使えない。**

## 決定

### 1. UI 操作はすべて `Input.dispatchMouseEvent` で行う

要素の座標を `getBoundingClientRect()` で取り、その中心に
press + release を送る。`element.click()` は使わない（実測 3）。

### 2. `Action::ClickFontSlider { fraction }` を `Action::SetFontSize { index }` に置き換える

[ADR-0005](0005-reader-display-settings.md) 決定 2 の「スライダー位置を割合で指定し、
毎回最小位置に戻してから目標位置へクリックする」手順は**不要になった。**

- `Observation` は現在のフォント段（`index` と `max`）を持つ
- `Action::SetFontSize { index }` は目標段を直接指定する
- `ke-cdp` は現在段と目標段の差だけ増減ボタンを押す（冪等）

**「設定 → 実測 → 検証」のループ自体は残す。** 段と画素/文字の対応は
書籍と viewport に依存するため、目標は依然「画素/文字」で指定する。
ただし探索は 0〜13 の離散 14 値に対する二分探索になり、**4 回以内に必ず収束する。**

### 3. ページ送りは chevron を `aria-label` で特定する

`#kr-chevron-left` / `#kr-chevron-right` のうち、`aria-label` が
「次のページ」に一致する方を「次」とする。**id や画面上の位置で決めない**
（縦書きと横書きで左右が入れ替わるため。実測 4）。

`aria-label` は表示言語に依存するので、日本語と英語の表記表を持つ。
**どちらにも一致しなければエラーで止める。** 推測して逆向きに撮り続けるより、
止まった方が良い。

### 4. 空振りを前提に、確定するまで再クリックする

一律の待ちを入れない。送ってから確定シグナル（位置表示の変化、または
blob URL の変化 + `complete`）を待ち、一定時間内に来なければ再クリックする（実測 6）。

### 5. 位置表示は「ページ」と「位置」を区別して記録する

`PageLabel` に種別を持たせる。**「位置」形式の書籍では巻末を確定できない**ため、
`Summary.end_confirmed` は `false` になる。これは
[ADR-0004](0004-page-acquisition.md) 決定 5 の「取りこぼしを把握できること」に沿う。

### 6. `Theme` は 4 値にする

観測できる現実を型で表せるようにする（`White` / `Dark` / `Sepia` / `Green`）。
OCR に使うのは `White` のままである。

## 影響

- **良い方向:** フォントサイズ設定が冪等になり、[ADR-0005](0005-reader-display-settings.md)
  決定 2 の「毎回最小位置まで戻す」手順が消えた。
  capture の実測所要時間が出た（**124〜265ms/頁**）ので、
  [ADR-0001](0001-architecture.md) の見積り（8.13s/頁 → 0.2〜0.4s/頁）は達成見込み
- **悪い方向:** ページ送りの向きが `aria-label` という**表示言語依存の文字列**に
  ぶら下がる。ここは将来必ず壊れる箇所なので、一致しなければ止める設計にした
- `ke-core` の `Action` / `Observation` / `Theme` / `PageLabel` と、
  `ke-nav` の `Calibrator` が変わる

## 未検証で残した項目

1. **フォント段と画素/文字の対応表。** [ADR-0005](0005-reader-display-settings.md)
   実測 3 は割合（5% / 35% / 65% / 95%）で測っており、段番号では測っていない。
   既定が段 5 で画素/文字 27 なのは一致するが、段 8〜12 あたりを実測して
   [ADR-0005](0005-reader-display-settings.md) 決定 3 の目標値と接続する必要がある。
   OCR 環境（NDLOCR-Lite）の再構築が要る
2. 「位置」形式の書籍で、先頭ページの位置が 1 になるか（巻き戻しの終端判定に効く）
3. 横書き（英語）書籍での chevron の `aria-label`
4. 空振りではなく「2 ページ飛んだ」が起きないか。位置の増分が一定でないため、
   validate フェーズで増分の外れ値を検出する価値がある
