# kindle-exporter

購入済み電子書籍を、**Mac でキャプチャし、Windows で OCR する**フェーズ分割型のエクスポートツール。
検索可能 PDF / NotebookLM 向け Markdown を出力します。

> **English:** A phase-based exporter for e-books you own. Capture runs on macOS (low power,
> long-running), OCR runs on Windows (GPU, short burst). Outputs searchable PDF and Markdown.
> Written in Rust (orchestration, browser driving, image pipeline) and Python (ONNX OCR, text layer).

---

## ステータス

**実装中。** ドメイン型（`ke-core`）とナビゲーション状態機械（`ke-nav`）が動いています。
ブラウザを実際に操作する層（`ke-cdp`）はこれからです。

- 設計判断とその根拠 → [docs/adr/](docs/adr/)（ADR-0001 〜 0006）
- **次に何をやるか** → [docs/ROADMAP.md](docs/ROADMAP.md)
- 実測に使った使い捨てスクリプト → [spikes/](spikes/)

設計は推測ではなく実機の実測に基づいて決めています。
主要な数値は [docs/ROADMAP.md](docs/ROADMAP.md) の「実測値の早見表」にまとめてあります。

## 何を解決するのか

既存の同種ツールは、OS のスクリーンショット API でリーダーの画面を撮り、
ピクセル差分でページ遷移を推測する方式が一般的です。この方式には構造的な問題があります。

- Win32 API に直接依存し、macOS で動かない
- 画面とマウスを占有するため、実行中は PC で他の作業ができない
- 「ページが変わったか」をピクセル差分で推測するため不安定
- 実機がないとテストが 1 行も書けない

本プロジェクトは、Kindle Cloud Reader が**ただのブラウザページである**ことを利用し、
**CDP（Chrome DevTools Protocol）**でブラウザを駆動します。これにより OS 依存がなくなり、
ページ遷移は DOM から確定的に取得でき、CDP をモックすればキャプチャ層が丸ごとテスト可能になります。

さらに実機調査の結果、Cloud Reader は**ページをサーバ側でレンダリングした画像として配信**
していることが分かりました（[ADR-0004](docs/adr/0004-page-acquisition.md)）。そのため
スクリーンショットを撮る必要すらなく、**ページ画像を原寸で直接取得**できます。

- UI（ヘッダー・矢印・進捗バー）が写り込まないため、**余白トリミングが不要**
- 表示サイズ 1001×1128 に対し**原寸 1501×1692** を取得できる
- 全画面化・ウィンドウ操作・マウス占有が一切不要（実行中も PC を普通に使える）

## 設計の要点

### フェーズ分割ワークフロー

```
plan ─→ open ─→ capture ─→ validate ─→ ocr ─→ proofread ─→ assemble
[any]  [browser] [browser]  [cpu]      [gpu]   [gpu/llm]    [cpu]
  │       └── Mac（低電力・長時間）──┘   └─── Windows（高性能・短時間）───┘
```

各フェーズは「入力アーティファクト → 出力アーティファクト」の独立した処理で、
必要なホスト能力（`browser` / `cpu` / `gpu`）を宣言します。
共有ストレージ上の追記専用イベントログで状態を持つため、
**マシンをまたいで中断・再開でき、機械間でパイプライン並列に動きます**
（Mac が N+1 冊目を撮っている間に Windows が N 冊目を OCR する）。

### 言語構成

| レイヤ | 言語 |
|---|---|
| CLI・オーケストレータ・フェーズ実行 | Rust |
| CDP ドライバ（ブラウザ駆動） | Rust |
| ナビゲーション状態機械（I/O なしの純粋関数） | Rust |
| 画像処理（検証・余白検出・トリム） | Rust + rayon |
| OCR 推論（NDLOCR-Lite / ONNX Runtime） | Python |
| PDF・Markdown 組み立て | Python |

Rust ↔ Python 間は JSON Lines over stdio。スキーマを単一の真実として両側から契約テストします。

### 日本語のルビと OCR 精度

日本語書籍のルビ（振り仮名）を正確に取り出すこと、および OCR の精度揺れへの対策を
設計要件としています。方針と実測データは
[docs/adr/0003-ruby-and-ocr-accuracy.md](docs/adr/0003-ruby-and-ocr-accuracy.md) にあります。

- **DOM に本文は存在しない** — 実機調査の結果、Cloud Reader はページを画像で配信して
  おり、OCR は回避できない（[ADR-0004](docs/adr/0004-page-acquisition.md)）
- **最大の精度レバーはリーダーのフォントサイズ** — 画素/文字を 27 → 51 に、ルビ列の幅を
  11px → 22px にできる。`open` フェーズで UI を操作して自動設定する
  （[ADR-0005](docs/adr/0005-reader-display-settings.md)）
- ルビ列は**分割せず一括認識**し（合成画像で 96%）、本文から得た読み候補と
  DP アライメントして相互に検証する
- **信頼度スコアは誤り検出に使わない** — 明らかな誤認識でも CONF が 0.93〜0.95 のまま
  だったため、低信頼度の箇所だけを校閲する設計は成立しない
- **校閲は独立フェーズ**とし、ローカル LLM には自由な書き換えをさせず、
  検証可能な**編集操作のリスト**を出力させて監査証跡を残す

### テスト方針

キャプチャ層は「実機がないとテストできない」ものとして扱いません。

- **ナビゲーションは I/O を持たない純粋な状態機械**（`step(Observation) -> Action`）として実装し、
  実機ゼロでユニットテスト・property test する
- **record / replay ハーネス** — 実行時の全 Observation / Action を JSONL に記録し、
  実機で起きた事故をそのまま回帰テストのフィクスチャにできる
- 循環的複雑度は CI で機械的に強制（clippy `cognitive_complexity` / ruff `C901`）

## 免責・利用上の注意

- 本ツールは、**利用者自身が購入した電子書籍**の私的複製（著作権法 30 条）を支援する目的で作られています
- 画面に表示された内容を取得する方式であり、**DRM の技術的保護手段を回避する機能は含みません**
- 一方で、Amazon Kindle をはじめとする各サービスの**利用規約に抵触する可能性があります**。
  利用は各自の責任と判断で行ってください
- 出力したファイルの再配布・共有・公開は行わないでください
- 本ソフトウェアは無保証で提供されます。詳細はライセンス条項を参照してください

### 商標について

Kindle および Amazon は Amazon.com, Inc. またはその関連会社の商標です。
本プロジェクトは Amazon とは一切関係がなく、承認・後援を受けているものでもありません。
プロジェクト名およびドキュメント中の商標への言及は、互換性を示すための指示的使用です。

## ライセンス

以下のいずれかを、利用者の選択により適用できます。

- Apache License, Version 2.0（[LICENSE-APACHE](LICENSE-APACHE) または <http://www.apache.org/licenses/LICENSE-2.0>）
- MIT License（[LICENSE-MIT](LICENSE-MIT) または <http://opensource.org/licenses/MIT>）

判断の経緯は [docs/adr/0002-license.md](docs/adr/0002-license.md) を参照してください。

### コントリビューション

明示的に別段の意思表示をしない限り、Apache-2.0 ライセンスに定義される、
本プロジェクトへの組み入れを意図して提出されたコントリビューションは、
追加の条件なしに上記のデュアルライセンスで提供されるものとします。

### サードパーティ

本プロジェクトは第三者のソフトウェアを利用・参照しています。
著作権者とライセンス条項は [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) を参照してください。
