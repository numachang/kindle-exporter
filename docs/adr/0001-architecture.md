# ADR-0001: 全体アーキテクチャ

- **ステータス:** 採択（未検証の前提あり — 「未解決の前提」節を参照）
- **日付:** 2026-08-08

## 背景

購入済み電子書籍 54 冊（12,221 ページ）を検索可能 PDF / NotebookLM 向け Markdown に
変換する既存ツール `kindle_shot`（MIT, Python 8,346 行）を評価した。

**良かった点:** 終了コードと JSON Lines イベントが契約として設計され契約テストで固定されている。
例外の握り潰しに理由が書かれている。コメントが実測値と日付を伴う。161 テストが 1.68 秒で全通過。
ruff クリーン。依存は `uv pip compile` で pin 済み。純ロジック層は素直に流用できる品質。

**問題点:**

1. Win32 API に直接依存し macOS で動かない
2. 12,221 ページに 27.6 時間（**8.13 秒/ページ**）
3. 最もリスクの高いキャプチャ層にテストが 1 件もない（作者も「Win32 実機依存のため対象外」と明記）
4. 循環的複雑度 10 超が 20 関数、最大 29（`reader_navigator._open_impl`）
5. 単一マシン前提で、処理を機械間で分割できない

### 要件

- macOS と Windows の両方で動く
- とにかく高速
- GPU を持つ Windows 機で重い処理を行う
- AI（OCR）は Python
- 複数言語の採用可
- 分解して複雑度を落とす
- **フェーズ別ワークフロー**（Mac で読み取り、Windows で OCR）
- 環境特性: Windows 機は高性能だが消費電力が大きい / Mac は非力だが低消費電力

## 決定

### 1. 画面キャプチャをやめ、CDP でブラウザを駆動する

Kindle Cloud Reader はブラウザページである。OS のスクリーンショット API とピクセル差分による
ページ遷移推測をやめ、**CDP（Chrome DevTools Protocol）**で Chrome を駆動する。

| | 従来（画面キャプチャ） | CDP |
|---|---|---|
| OS 依存 | Win32 API 直叩き | なし（WebSocket + JSON） |
| macOS 対応 | ScreenCaptureKit を別実装 | そのまま動く |
| 画面の占有 | 前面必須・マウス操作禁止 | 不要 |
| macOS の権限 | 画面収録 + アクセシビリティ | 不要 |
| ページ遷移の検出 | ピクセル差分による推測 | DOM / canvas を JS で観測して確定 |
| キー送信 | pyautogui（物理キーボード相当） | `Input.dispatchKeyEvent`（対象タブに直接） |
| テスト | 実機必須 | CDP をモックすれば全て可能 |

これにより `kindle_shot` で最も複雑だった処理 —「白ページか読み込み中かの判別」
「位置同期モーダルの黒矩形を画像認識で探してクリック」「リーダー UI トグルを左右端の
暗ピクセル差分で判定」— が**すべて不要になる**。あれらは画面キャプチャ方式を選んだことの
帰結であって、本質的な複雑さではなかった。

`ScreenSource` trait を切っておき、ブラウザ版が存在しないリーダー向けの
ネイティブキャプチャ backend を後から差し込めるようにする。

### 2. フェーズ分割ワークフロー

```
plan ──→ open ──→ capture ──→ validate ──→ trim ──→ ocr ──→ assemble
[any]    [browser] [browser]   [cpu]        [cpu]    [gpu]   [cpu]
  │         └──── Mac（低電力・長時間）────┘         └── Windows（高性能・短時間）──┘
```

各フェーズは入力アーティファクトから出力アーティファクトを作る独立した処理とし、
必要なホスト能力（`browser` / `cpu` / `gpu`）を宣言する。

### 3. 状態は共有ストレージ上の追記専用イベントログで持つ

```
library/
  <book-id>/
    manifest.json          # ASIN, タイトル, 形式, 設定
    events.jsonl           # 追記専用のフェーズ遷移ログ（唯一の真実）
    pages/raw/0001.png     # capture の出力
    pages/trimmed/0001.png # trim の出力
    text/0001.json         # ocr の出力（行 + bbox + 信頼度）
    out/<title>.pdf
    .lease/ocr             # O_EXCL で作る排他ロック
```

**単一の SQLite に状態を持たせない。** ネットワーク共有（SMB / Syncthing）上の SQLite は
ロックが正しく動かない。書籍ごとの追記専用 JSONL なら衝突せず、マージ可能で、
オフラインでも進行できる。各ホストのローカル SQLite は再構築可能な索引に留める。
コーディネータのデーモンやサーバは置かない。

この設計の帰結として、**Mac が N+1 冊目をキャプチャしている間に Windows が N 冊目を OCR できる**
（機械間のパイプライン並列）。片方が落ちても、電源を切っても、そこから再開できる。

### 4. 言語構成

| レイヤ | 言語 | 理由 |
|---|---|---|
| CLI・オーケストレータ・フェーズ実行 | Rust | 両 OS で単一バイナリ、ランタイム不要 |
| CDP ドライバ | Rust（自前の薄い WS クライアント） | 使うのは Target/Page/Input/Runtime の 10 メソッド程度。自前だとモックが書ける |
| ナビゲーション状態機械 | Rust（I/O なしの純粋関数） | テストの中心 |
| 画像処理（検証・余白検出・トリム） | Rust + rayon | 12,221 枚は完全並列 |
| OCR 推論 | Python + onnxruntime | 常駐ワーカー。CUDA EP (Win) / CoreML EP (Mac) |
| PDF・Markdown 組み立て | Python | kindle_shot の実績あるコードを流用 |

Rust ↔ Python は **JSON Lines over stdio**。JSON Schema を単一の真実として両側から契約テストする。
gRPC のツールチェーンは導入しない。

**Go を採用しない理由:** 画像処理が Rust の 2〜3 倍遅く、ネイティブキャプチャへの
フォールバックが必要になった時点で cgo が入り Go の利点が消える。

**画像処理を GPU に載せない理由:** トリミング・余白検出・検証は 12,221 枚でも CPU 並列で
数分で終わる（1 枚 5〜15ms × 16 スレッド）。GPU が本当に効くのは OCR 推論だけ。
したがって「Windows で処理する理由」は GPU ではなく**単に速いマシンだから**と整理する。
この整理により、GPU が使えない環境でも CPU EP にフォールバックするだけで動作する。

### 5. crate 構成（crate 境界 = テスト継ぎ目）

```
crates/
  ke-core/      ドメイン型のみ。依存ゼロ
  ke-cdp/       CDP クライアント + trait Browser（+ FakeBrowser）
  ke-nav/       純粋状態機械。ke-core にしか依存しない
  ke-imaging/   検証・余白検出・トリム（rayon）
  ke-store/     アーティファクト配置・events.jsonl・lease
  ke-workflow/  フェーズグラフ・実行・再開
  ke-cli/       clap。単一バイナリ `ke`
py/
  ke_ocr/       常駐 OCR ワーカー（onnxruntime）
  ke_text/      kindle_shot 由来の text / PDF 層
```

### 6. テスト戦略

`kindle_shot` が「プロレベルでない」と判断された理由は、最も壊れるキャプチャ層に
テストが 1 行もないことだった。これを設計で解く。

**a. ナビゲーションを I/O のない純粋な状態機械にする**

```rust
fn step(&mut self, obs: Observation) -> Action
// Observation = { page_index: Option<u32>, canvas_ready: bool, modal: Option<Modal>, ... }
// Action = PressNext | PressPrev | Click(x,y) | Capture | WaitMs(u32) | Done | Fail(reason)
```

時計もファイルもネットワークも触らない。巻き戻しの終端判定・白ページ・モーダル・UI トグルの
全分岐が実機ゼロでユニットテストおよび property test 可能になる。

**b. record / replay ハーネス（最重要）**

`ke capture --record` が全 Observation と全 Action を JSONL に落とす。
実機で事故が起きたらそのセッションログを `fixtures/sessions/` に置けば、そのまま回帰テストになる。
`kindle_shot` が構造的にできなかったのはこれであり、本設計の主眼はここにある。

**c. その他**

- `ke-imaging`: 合成ページ生成 + golden image（kindle_shot の `conftest.py` と同じ発想）
- `ke-workflow`: tmpdir にアーティファクトを作り、途中で kill して再開できるかを検証
- Python ワーカー: スタブ ONNX セッションでプロトコル契約テスト
- クロス言語: ワーカープロトコルの JSON Schema を Rust 側と Python 側の両方から検証
- E2E: 20 ページの「ゴールデン本」を fake CDP + stub OCR で全フェーズ通す

**d. 複雑度の機械的強制**

CI で clippy `cognitive_complexity`（閾値 10）と ruff `C901` をエラー扱いにする。
「20 関数が閾値超過、最大 29」を再発させない。

### 7. kindle_shot からの流用範囲

| 捨てる | 流用する（MIT・要出典明記） |
|---|---|
| `win32_utils.py`（Win32 直叩き・テストゼロ） | `text_reflow.py`（段落整形） |
| `capture_engine.py`（ピクセル差分） | `text_cleanup.py` / `text_patterns.py` |
| `reader_navigator.py`（複雑度 29） | `chapter_detector.py`（章検出） |
| `ui/`（customtkinter GUI） | `markdown_writer.py`（NotebookLM 最適化） |
| `cli.py`（36KB 単一ファイル） | `pdf_builder.py`（検索可能 PDF の透明テキスト層） |

「機械を触る層」を Rust で作り直し、「テキストを扱う層」は Python のまま流用する。
テキスト系のテストもそのまま持ち込む。

## 期待される結果

### 速度

現行 12,221 ページ / 27.6 時間 = **8.13 秒/ページ**。

| 工程 | 現行 | 原因 | 対策 | 目標 |
|---|---|---|---|---|
| ページ遷移待ち | 約 5〜6s | 固定 sleep + 1.2s 間隔ポーリング | JS で描画完了を確定検出 | 0.2〜0.4s |
| 画面取得 | 約 0.2s | PIL ImageGrab | CDP clip 撮影 | 0.05s |
| PNG エンコード | 約 0.3s | 同期・フル解像度 | 別スレッド、critical path 外 | 0s |
| OCR | 1.3s/p + モデルロード 19s × チャンク数 | CLI をチャンク毎に起動 | 常駐ワーカー + バッチ推論 | 0.2〜0.4s |
| トリミング等 | 全ページ 2 パス走査 | Python 逐次 | Rust + rayon | 数分/全体 |

NDLOCR-Lite は ONNX Runtime ベース（DEIMv2 + PARSeq + 読み順ソート、ONNX 4 本で約 150MB）。
CLI 経由をやめて `onnxruntime` を直接呼ぶことで、(1) モデルロードが 1 回で済む、
(2) PARSeq は行単位認識器なので 1 ページ 20〜40 行を 1 バッチにまとめられる、
(3) CUDA EP が使える、の 3 点が効く。

**総合目標: 27.6h → 3〜4h**（キャプチャと OCR を機械間でオーバーラップさせた実時間）。

### 消費電力（概算）

| | 現行 | 本設計 |
|---|---|---|
| キャプチャ | Windows 27.6h × 約 120W = 約 3.3 kWh | Mac 2.5h × 約 15W = 約 0.04 kWh |
| OCR | 上に含む | Windows 1.2h × 約 250W = 約 0.30 kWh |
| 合計 | **約 3.3 kWh** | **約 0.34 kWh（約 1/10）** |

Windows 機を「27 時間つけっぱなし」から「1 時間だけ全力」に変えられることが本質。

## 未解決の前提（着手前に検証する）

実装前に以下 4 件をスパイクで検証する。結果次第で構成が変わる。

1. **Cloud Reader の canvas が CDP の `Page.captureScreenshot` で取得できるか。**
   黒塗りになる場合はネイティブキャプチャ backend にフォールバックする
   （`ScreenSource` trait を切ってあるため上位層は変更不要）。

   > **[ADR-0003](0003-ruby-and-ocr-accuracy.md) により拡張:** このスパイクは
   > 「撮影可否」だけでなく **「DOM に本文とルビが実体として存在するか」** を
   > 確認するものに変更された。存在する場合、リフロー型書籍については OCR 経路
   > そのものが不要になる（本文精度 100%・ルビは `<ruby><rt>` から構造ごと取得）。
2. **ページ描画完了を JS で確定検出できるか。** DOM にページ番号や canvas の更新シグナルがあるか。
   無い場合は縮小フレームの安定判定にフォールバックする。
3. **`onnxruntime-gpu` の CUDA EP で NDLOCR-Lite の 4 モデルが動くか。**
   上流では `--device cuda` は beta 扱い。動かない場合も常駐化とバッチ推論の効果は残る。
4. **Apple Silicon の CoreML EP（Neural Engine）で OCR がどの程度出るか。**
   M シリーズが 15W 級で実用速度を出すなら、Windows 機を起こさず Mac だけで完結する
   選択肢が生まれる。消費電力の前提が変わるため測定する価値がある。

1 と 3 の結果でアーキテクチャの骨格が確定する。

## 影響

- **良い方向:** OS 依存の消滅、テスト可能性の獲得、機械間パイプライン並列、消費電力 1/10、
  実行中に PC を占有しない
- **コスト:** Rust + Python の 2 言語構成となり、クロス言語の契約テストが必要になる。
  Chrome への依存が新たに生まれる（ユーザーは手動でログインした Chrome を用意する）
- **リスク:** CDP でのキャプチャ可否が未検証（上記スパイク 1）。
  ここが NG の場合、macOS 向けに ScreenCaptureKit backend の実装が追加で必要になる
