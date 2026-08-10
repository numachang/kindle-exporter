# ROADMAP — 残りの作業

2026-08-09 時点。設計判断は [ADR](adr/) に、実測の再現手順は [spikes/](../spikes/) にある。
本書は**次に何をやるか**だけを書く。

---

## 現在地

| | 状態 |
|---|---|
| 設計 | ADR-0001 〜 0007 で確定。未検証項目は下記「残っている不確実性」 |
| `ke-core` | **完了**（ドメイン型、テスト 36 件） |
| `ke-nav` | **完了**（純粋状態機械 + フォント校正、テスト 32 件） |
| `ke-cdp` | **完了**（`trait Browser` + `FakeBrowser` + CDP クライアント + 記録/再生、テスト 40 件） |
| `ke-store` | **完了**（配置・追記専用イベントログ・lease、テスト 17 件） |
| `ke-workflow` | capture フェーズ **完了**（テスト 12 件）。他のフェーズは未着手 |
| `ke-imaging` / `ke-cli` | 未着手 |
| Python 側（`ke_ocr` / `ke_text`） | 未着手 |
| CI | Rust 3 OS + Python（`py/` が出来たら自動で有効化）すべて green |

`ke-nav` の決定を `ke-cdp` が実行できるようになった。
実機で観測・取得・表示設定・ページ送りがすべて通ることを確認済み
（`cargo run -p ke-cdp --example probe`）。

**実機から保管庫まで一本で通るようになった。**
`cargo run -p ke-workflow --example shoot -- <ASIN> <保管庫> [枚数]` で、
本を開き、表示設定を確定し、先頭へ巻き戻し、ページ画像を
`library/<ASIN>/pages/raw/0001.png` に保存し、経過を `events.jsonl` に残す。
途中で落ちても、もう一度走らせれば撮り切る。

**残っているのは画素/文字の実測（OCR）だけである。** それが無いので
フォントサイズ校正は仮の値で走っており、`px_per_char` は測定結果ではない。

> **Chrome の起動フラグが必須。** `--disable-backgrounding-occluded-windows` 等を
> 付けないと、ウィンドウを最小化・被覆した時点で capture が実質停止する
> （ADR-0007 実測 10）。手順は下記「環境の再構築」。

---

## 1. record / replay ハーネス — **完了**

ADR-0001 §6b。`ke-cdp` の [`Session`] / [`ReplayBrowser`] として実装した。

- 1 手（[`Observation`] と [`Action`] の対）を 1 行の JSON Lines に落とす
- [`ReplayBrowser`] が実機なしで再生し、`Session::diverged_from` が
  **何手目で判断が変わったか**を報告する
- `crates/ke-cdp/fixtures/sessions/*.jsonl` に置いたものは CI で自動的に検証される。
  **実機で事故が起きたら、その記録をここに置くだけで回帰テストになる**

```bash
# 実機を本番ループで回して記録する（ページ画像は保存しない）
cargo run -p ke-cdp --example probe -- capture <ASIN> 15 out.jsonl
# フィクスチャを作り直す
cargo test -p ke-cdp --test browser -- --ignored regenerate
```

**記録するのは観測と行動だけで、ページ画像は記録しない**（書籍の中身なので）。
保存時に `Session::redacted` で ASIN を伏せる。CI がそれを機械的に検査する。

> **既知の制約:** 記録は**同じ `Limits` でしか再生できない**。打ち切り条件が
> 変わると判断も変わるため。記録側に条件を持たせるのは今後の課題。

早速このハーネスが 2 つ仕事をした。1 つはページ送りの間隔を実測から
180ms に決めたこと（1 枚 492ms → 227ms。ADR-0007 実測 12）。
もう 1 つは、その変更でフィクスチャの判断が変わったことを CI が検出したこと。

---

## 2. `ke-store` — アーティファクトとイベントログ — **完了**

ADR-0001 §3 の配置を実装した。

```
library/<ASIN>/
  manifest.json          BookSpec
  events.jsonl           追記専用のイベントログ（唯一の真実）
  pages/raw/0001.png     capture の出力
  sessions/*.jsonl       記録（1 の record/replay）
  .lease/capture         フェーズの排他ロック
```

- **単一 SQLite を置かない。** ネットワーク共有上ではロックが壊れる
- ページ画像は一時ファイルに書いてから改名する。**中途半端な PNG を残さない**
- lease は `O_EXCL` で作るファイル 1 個。**残ったロックを自動で奪わない**
  （落ちたホストと二重に走るため）。誰が掴んでいるかを返して人間に判断させる
- 進捗はイベントから組み立てる。同じ連番を数え直さないので、撮り直しても矛盾しない

## 3. `ke-workflow` — capture フェーズ **完了**、他は未着手

判断（`ke-nav`）・実行（`ke-cdp`）・保管（`ke-store`）・実測（[`Measurer`]）を
噛み合わせる層。**この層自身は何も判断しない。**

capture は実装済みで、実機で 30 枚を保管庫に落とすところまで通っている。

> **再開は「途中から」ではなく「先頭から撮り直す」。** ページの連番は
> 先頭から数えた枚数なので、同じ表示設定なら上書きは冪等になる。
> N ページ目まで進めるにも結局 N 回送るので、差は画像取得の 17ms 分しかない。
> 200 ページの撮り直しは 45 秒。落ちたときだけ払う費用としては安い。

残り:

- **`Measurer` の本実装**（4 と連動）。いまは `StubMeasurer` で走っており、
  記録される `px_per_char` は測定結果ではない
- validate / ocr / proofread / assemble の各フェーズ
- フェーズグラフと、能力（`browser` / `cpu` / `gpu`）によるホストの振り分け

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

## 8. `ke-imaging` / `ke-cli`

- **`ke-imaging`**: 白紙・重複・サイズ違いの検証だけ。**余白検出は不要**
  （ADR-0004 決定 2 で `trim` フェーズを削除した）。rayon で並列化。
  `Summary.end_confirmed` が `false` の書籍を警告する
- **`ke-cli`**: clap。単一バイナリ `ke`。
  `crates/ke-workflow/examples/shoot.rs` が原型になっている

### 生ページ画像の容量と、いつ捨てられるか

実測 1 枚 **784KB**（2199x1692 の PNG）。蔵書 12,221 ページに対して:

| 条件 | 枚数 | 生 PNG |
|---|---|---|
| フォント段 6（現状の実機設定） | 12,221 | 約 9.4GB |
| **画素/文字 45（ADR-0005 の既定目標）** | **約 41,500** | **約 32GB** |

精度を上げると枚数が 3.4 倍になるので（ADR-0005 影響）、**本番では 32GB 前後**を見る。

**捨てられるのは `proofread` の後であって、`ocr` の後ではない。**

- `proofread` は編集案を画像と突き合わせて棄却する（上記 6）
- ルビの「未確定」箇所は画像を切り出して再検出する（上記 5 の 7、ADR-0004 決定 5）

したがって生画像は capture から proofread まで生き残る必要があり、
しかも capture は Mac・OCR は Windows なので、その間は共有ストレージに載る。

`ke-cli` は**書籍単位で capture → ocr → proofread → 生画像を捨てる**と回せるように
作る。そうすればピークは同時に処理中の冊数ぶんで済み、全蔵書ぶんを一度に
置かずに済む。`ke-store` に「生画像を捨てる」操作と、
捨てたことをイベントに残す仕組みが要る（未実装）。

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
| 1 | **1 冊の完走**。30 枚までは通した。巻末まで一気に撮り切るのは未実施 | 所要時間の総計が出ない |
| 2 | **フォント段と画素/文字の対応表**。ADR-0005 は割合で測っており段番号では測っていない | 既定の目標段が決まらない。OCR 環境の再構築が要る |
| 3 | `sideMarginsSize` / `maxNumberColumns` を UI から変えた効果。操作子は `#narrow` / `#medium` / `#wide` と特定済み | 同じフォントサイズでより多くの文字が入る可能性 |
| 4 | マンガ（固定レイアウト）でも同じ `kg-full-page-img` 経路か | 経路分岐が要るかどうか |
| 5 | 洋書・リフロー型英語書籍では DOM にテキストがあるか。chevron の `aria-label` も要確認 | あれば書籍種別による分岐に価値が出る |
| 6 | アクセシビリティ API（Kindle for PC + UI Automation） | 日本語書籍では見込み薄。洋書があれば確認価値あり |

---

## 環境の再構築

`ghq get` で clone し直したあと、この 3 つは手元に作り直す必要がある。

### Chrome の専用プロファイル

Amazon にログイン済みの専用プロファイルが要る。
**Chrome 136 以降は既定プロファイルだと `--remote-debugging-port` が無視される**ため、
`--user-data-dir` の指定が必須。置き場所は temp ではなく永続的な場所にする
（例: `~/.ke-chrome-profile`）。

```powershell
& "C:\Program Files\Google\Chrome\Application\chrome.exe" `
  --remote-debugging-port=9222 --remote-allow-origins=* `
  --user-data-dir="$env:USERPROFILE\.ke-chrome-profile" `
  --no-first-run --no-default-browser-check `
  --disable-backgrounding-occluded-windows `
  --disable-renderer-backgrounding `
  --disable-background-timer-throttling `
  "https://read.amazon.co.jp/"
```

**下 3 つのフラグは必須。** 無いと、ウィンドウが隠れた瞬間に Chrome が
レンダラを絞り、1 ページ 1.8 秒に落ちてやがて止まる（ADR-0007 実測 10）。

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
| **本番ループ 1 ページ**（`ke-nav` + `ke-cdp`） | **227ms/頁** | ADR-0007 実測 12 |
| 反映を見る間隔 | **180ms が底**。短いと空振りを買って逆に遅い | ADR-0007 実測 12 |
| 巻末・先頭の確定シグナル | chevron が **DOM から消える**（矩形 0 は「隠れている」だけ） | ADR-0007 実測 11 |
| Chrome の絞り込み無効フラグ | **必須**。無いと隠れた瞬間に 1.8 秒/頁へ | ADR-0007 実測 10 |
| **送りの最小間隔** | **100ms。これより短いと黙って無視される** | ADR-0007 実測 8 |
| 押し直しまでの最適値 | 400ms | ADR-0007 実測 8 |
| 観測（`Runtime.evaluate`）の下限間隔 | 20ms。詰めるとページが遅くなる | ADR-0007 実測 9 |
| 表示設定変更後の再描画 | 2.8 秒（その間ページ画像は DOM から消える） | ADR-0007 実測 7 |
| フォント段 | 0〜13 の 14 段。既定は 5。`value` **属性**から読める | ADR-0007 実測 2 |
| ページ画像の取り出し（canvas → PNG） | 17ms / 約 600KB | ADR-0007 実測 6 |
| 保存した PNG 1 枚 | **784KB**（2199x1692） | ke-workflow で 30 枚実測 |
| 全蔵書の生 PNG | 約 9.4GB（現設定）/ **約 32GB**（既定目標） | 上と ADR-0005 影響 |
| `createIsolatedWorld` / `Runtime.evaluate` | 1.2ms / 0.3ms | ADR-0007 実測 6 |
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
[`Session`]: ../crates/ke-cdp/src/record.rs
[`ReplayBrowser`]: ../crates/ke-cdp/src/record.rs
[`Measurer`]: ../crates/ke-workflow/src/measure.rs

---

## 実機に繋いで確かめる

Chrome を専用プロファイルで起動し、本を開いた状態で:

```bash
cargo run -p ke-cdp --example probe                # いま観測できることを表示
cargo run -p ke-cdp --example probe -- capture     # ページ画像を 1 枚取り出す
cargo run -p ke-cdp --example probe -- settings 9  # 白テーマ + フォント段 9 にする
cargo run -p ke-cdp --example probe -- turn 20     # 20 ページ送って所要時間を測る

# 1 冊を撮って保管庫に置く（本番と同じ経路）
cargo run -p ke-workflow --example shoot -- <ASIN> <保管庫のパス> [枚数の上限]
```

セレクタは実機の DOM に依存しているので、リーダーの UI が変わるとここが最初に壊れる。
