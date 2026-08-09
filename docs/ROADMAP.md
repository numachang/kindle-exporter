# ROADMAP — 残りの作業

2026-08-09 時点。設計判断は [ADR](adr/) に、実測の再現手順は [spikes/](../spikes/) にある。
本書は**次に何をやるか**だけを書く。

---

## 現在地

| | 状態 |
|---|---|
| 設計 | ADR-0001 〜 0006 で確定。未検証項目は下記「残っている不確実性」 |
| `ke-core` | **完了**（ドメイン型、テスト 31 件） |
| `ke-nav` | **完了**（純粋状態機械 + フォント校正、テスト 24 件） |
| `ke-cdp` | 未着手 ← **次はここ** |
| `ke-imaging` / `ke-store` / `ke-workflow` / `ke-cli` | 未着手 |
| Python 側（`ke_ocr` / `ke_text`） | 未着手 |
| CI | Rust 3 OS + Python（`py/` が出来たら自動で有効化）すべて green |

現状で `ke-nav` は「本を開く → 表示設定を確定 → 先頭へ巻き戻し → 全ページ撮影」を
すべて決定できるが、**その決定を実行する相手がいない**。それが `ke-cdp`。

---

## 1. `ke-cdp` — ブラウザ抽象と CDP クライアント

`ke-nav` が返す [`Action`] を実際のブラウザ操作に変換する層。

### 設計方針（決定済み）

- **async を使わない。** ブラウザ 1 タブを逐次操作するだけなので `tungstenite` の
  ブロッキング API で足りる。`ke-nav` も同期なので噛み合う。tokio を持ち込まない
- `trait Browser` を切り、`FakeBrowser` を同梱する。これで `ke-workflow` 以降も
  実機ゼロでテストできる
- HTTP（`/json/version`、`/json/list`）は `ureq` 程度の軽いもので足りる

### 実装する操作

`spikes/cdp_grab.py` と `spikes/font_control.py` がそのまま参照実装になる。

| `Action` | CDP 呼び出し |
|---|---|
| `OpenBook` | `Page.navigate` |
| `OpenSettingsMenu` / `CloseSettingsMenu` | `Runtime.evaluate`（`aria-label="リーダー設定"` を click） |
| `SetTheme` | 設定メニュー内の UI 操作（**未特定。要調査**） |
| `ClickFontSlider` | `Input.dispatchMouseEvent`（`ion-range` の矩形 × 割合） |
| `PressNext` / `PressPrev` | `Input.dispatchKeyEvent` |
| `CapturePage` | isolated world + canvas → PNG バイト列 |
| `MeasurePage` | 画像を取って Python 側に測らせる（→ 4 と連動） |
| 観測 | `Page.getFrameTree` → `Page.createIsolatedWorld` → `Runtime.evaluate` |

### 落とし穴（実測済み。忘れると詰まる）

- **isolated world 必須。** 素の `fetch(blobURL)` は Amazon が `window.fetch` を
  差し替えているため `TypeError: Failed to fetch` になる
- CDP のパラメータ名 `grantUniveralAccess` は**プロトコル側の綴り間違い**。直さない
- メニュー開閉は `ion-menu` の `show-menu` クラスで判定する。`ion-backdrop` は出ない
- `ion-range` の `value` は JS から読めない。だから「設定 → 実測 → 検証」が要る

### 未特定

**テーマを白にする UI 操作が未特定。** 設定メニュー内にテーマ選択があるはずだが、
`spikes/open_settings.py` の出力では操作子を特定できていない。
`ke-cdp` 実装時に設定メニューの DOM を掘る必要がある。
最悪、テーマが変えられなくても画像を反転すれば動く（ADR-0004 の元の方針に戻すだけ）。

---

## 2. record / replay ハーネス

ADR-0001 §6b。**本設計の主眼**であり、`ke-cdp` の直後に作る。

- `ke capture --record <path>` で全 [`Observation`] と [`Action`] を JSONL に落とす
- `FakeBrowser` が JSONL を再生する
- 実機で事故が起きたらそのログを `fixtures/sessions/` に置くだけで回帰テストになる

[`Observation`] と [`Action`] は既に JSON で往復できる（`ke-core` のテストで固定済み）。

---

## 3. `ke-store` — アーティファクトとイベントログ

ADR-0001 §3 の配置を実装する。

```
library/<book-id>/
  manifest.json          # BookSpec
  events.jsonl           # 追記専用のフェーズ遷移ログ（唯一の真実）
  pages/raw/0001.png
  text/0001.json
  out/<title>.pdf
  .lease/<phase>         # O_EXCL で作る排他ロック
```

- **単一 SQLite を置かない。** ネットワーク共有上ではロックが壊れる
- ローカル SQLite は `events.jsonl` から再構築可能な索引に留める
- テスト: tmpdir に作って途中で kill し、再開できることを確認する

---

## 4. Python 側 — `ke_ocr` 常駐ワーカー

- `onnxruntime` を直接呼ぶ。**CLI（`src/ocr.py`）は使わない**
  （モデルロード 19 秒 × チャンク数が丸ごと無駄になる）
- 検出（DEIM）は **GPU**、認識（PARSeq）は **CPU 8 並列**（ADR-0003 実験 2）
- Rust とは **JSON Lines over stdio**。スキーマを 1 本置いて両側から契約テスト
- `MeasurePage` の実装もここ（画素/文字＝縦書きなら本文行の幅の中央値）

`spikes/gpu_diag.py` に CUDA EP のセットアップ手順がある（cuDNN の pip パッケージと
`os.add_dll_directory` が要る。忘れると黙って CPU に落ちる）。

将来の最適化候補: PARSeq の ONNX は Reshape にバッチ 1 がハードコードされていて
動的バッチ化できない。PyTorch 重みが入手できれば再エクスポートで GPU バッチ推論が
可能になり、認識時間を大幅に短縮できる見込み。

---

## 5. ルビ復元パイプライン

ADR-0003 決定 2 + ADR-0005 決定 4。

1. `BLOCK TYPE="ルビ"` の bbox を採用する（NDLOCR-Lite 本体は捨てている）
2. **ルビ列を分割せず、列全体を 1 本の文字列として PARSeq に通す**（実測 96%）
3. 列内の空白位置からルビ群の Y 範囲を取る（**位置情報としてのみ使う**）
4. 本文行を形態素解析して各漢字列の読み候補を得る
5. **照合前に小書き仮名を正規化する**（`ゃゅょっぁぃぅぇぉ` → `やゆよつあいうえお`）。
   NDLOCR は拗音を大書きで書く近代資料で学習されているため、`じゅしき` が
   `ちゆじゆしき` になる系統的な偏りがある。正規化で機械的に吸収できる
6. 読み候補列と DP アライメントし、**成立したら辞書側の表記を採用する**
7. 破綻した箇所は捨てずに「未確定ルビ」として記録し、校閲フェーズへ送る

形態素解析器（読み付き）への依存が増える。OCR 経路にのみ必要。

**ルビの完全性は保証しない。** 保証するのは「出力したルビが正しいこと」と
「取りこぼしを把握できること」（ADR-0004 決定 5）。

---

## 6. `proofread` フェーズ — ローカル LLM 校閲

ADR-0003 決定 4。

- **信頼度で対象を絞らない。** 明らかな誤認識でも CONF が 0.93〜0.95 のままだった
- LLM に自由記述をさせない。出力は**編集操作のリスト**（位置・変更前・変更後・理由・確信度）
- 各編集を機械検証する（編集距離が閾値超過、画像と矛盾 → 自動棄却）
- `text/<page>.corrections.json` に全編集を記録。原文も保持する
- ルビの「未確定」箇所を優先的に提示する
- モデルは差し替え可能に。既定は実装時にベンチマークして決める（12GB VRAM に収まるもの）

---

## 7. `ke_text` — 組み立て（`kindle_shot` から流用）

MIT。出典は [THIRD-PARTY-NOTICES.md](../THIRD-PARTY-NOTICES.md) に記載済み。
派生ファイルの冒頭に由来コメントを入れること。

| 流用元 | 用途 |
|---|---|
| `core/text_reflow.py` | OCR 後の改行結合・段落自動整形 |
| `core/text_cleanup.py` / `core/text_patterns.py` | 行内クリーニング |
| `core/chapter_detector.py` | 章見出しの自動検出 |
| `core/markdown_writer.py` | Markdown 出力（NotebookLM 最適化） |
| `core/pdf_builder.py` | 検索可能 PDF の透明テキスト層 |

テストも一緒に持ち込む。

---

## 8. `ke-imaging` / `ke-workflow` / `ke-cli`

- **`ke-imaging`**: 白紙・重複・サイズ違いの検証だけ。**余白検出は不要**
  （ADR-0004 決定 2 で `trim` フェーズを削除した）。rayon で並列化
- **`ke-workflow`**: フェーズグラフ、lease、再開。`Summary.end_confirmed` が `false` の
  書籍を `validate` で警告する
- **`ke-cli`**: clap。単一バイナリ `ke`

---

## 9. `plan` フェーズ

- 蔵書一覧 → `books.json`。`Xetera/kindle-api`（Node.js から Kindle の内部 API を叩く）が
  使えるか検討する
- **技術書と判定した書籍には「DRM フリー版が存在する可能性がある」と出力する**
  （ADR-0006 決定 3）。達人出版会・技術評論社・オライリー等。
  OCR より正確で、校閲もルビ復元も不要になる。判定は出版社名のヒューリスティックで足りる

---

## 残っている不確実性

実装しながら測るのが早い段階に来ている。

| # | 内容 | 影響 |
|---|---|---|
| 1 | **capture の実測所要時間**。既定設定で撮影枚数が約 3.4 倍になるため、capture が全体の律速になる見込み | ADR-0001 の「27.6h → 3〜4h」を測り直す必要がある |
| 2 | テーマを白にする UI 操作の特定 | 出来なければ画像反転に戻すだけ |
| 3 | `sideMarginsSize` / `maxNumberColumns` を UI から変えた効果 | 同じフォントサイズでより多くの文字が入る可能性 |
| 4 | マンガ（固定レイアウト）でも同じ `kg-full-page-img` 経路か | 経路分岐が要るかどうか |
| 5 | 洋書・リフロー型英語書籍では DOM にテキストがあるか | あれば書籍種別による分岐に価値が出る |
| 6 | アクセシビリティ API（Kindle for PC + UI Automation） | 日本語書籍では見込み薄。洋書があれば確認価値あり |

---

## 環境の再構築

`ghq get` で clone し直したあと、この 3 つは手元に作り直す必要がある。

### Chrome の専用プロファイル

Amazon にログイン済みの専用プロファイルが要る。
**調査時のものは Windows の temp 配下（`%TEMP%\ke-chrome-profile`）に作ったため、
いずれ消える。** 残したいなら退避すること。

作り直す場合は [spikes/README.md](../spikes/README.md) の手順で起動して再ログインする。
本番でも同じ「専用プロファイル + `--remote-debugging-port`」を使うので、
temp ではなく永続的な場所（例: `~/.ke-chrome-profile`）に置くのが望ましい。

### Python 環境

[spikes/README.md](../spikes/README.md) 参照。NDLOCR-Lite の取得と、
GPU を使う場合の NVIDIA ランタイム（`os.add_dll_directory` が必要）。

### 参考にした派生元

`kindle_shot` は note.com の記事から ZIP で配布されている。
**リポジトリには含めていない**（ライセンス表記が不完全な他人の著作物のため）。
テキスト層を流用する際に取得する。出典は
[THIRD-PARTY-NOTICES.md](../THIRD-PARTY-NOTICES.md) に記載済み。

---

## 実装時に効く実測値の早見表

| 項目 | 値 | 出典 |
|---|---|---|
| ページ画像の原寸 | viewport × 1.2（`deviceScaleFactor` は効かない） | ADR-0004 実測 5 |
| 画素/文字（既定フォント） | 27〜30。viewport を変えても不変 | ADR-0004 実測 5 |
| 画素/文字（スライダー 65% / 95%） | 45 / 51 | ADR-0005 実測 3 |
| ルビ列の幅（同上） | 20px / 22px。PARSeq は高さ 24px に正規化するので 22px がほぼ理想 | ADR-0005 実測 3・4 |
| 文字/ページ（同上） | 536 / 441（既定フォントでは 1796） | ADR-0005 実測 3 |
| 行 CONF | 0.92〜0.93 | ADR-0004 実測 6 |
| DEIM 検出 | CPU 235.6ms / **GPU 23.9ms**（9.9 倍） | ADR-0003 実験 2 |
| PARSeq 認識 | CPU 33.2ms / GPU 46.1ms（**GPU の方が遅い**） | ADR-0003 実験 2 |
| PARSeq CPU 並列 | 8 並列で 39.3 行/秒（16 並列は頭打ち） | ADR-0003 実験 2 |
| ルビ一括認識の精度 | 合成画像で 106/110 ≈ 96% | ADR-0003 実測 3 |

[`Action`]: ../crates/ke-core/src/action.rs
[`Observation`]: ../crates/ke-core/src/observation.rs
