# Third-Party Notices

本プロジェクトが利用・参照する第三者ソフトウェアと、その著作権者・ライセンス条項の一覧です。

Rust / Python の推移的依存については、実装着手後に `cargo about` および `pip-licenses` で
自動生成した一覧を本ファイルに追記します。

---

## kindle_shot

- **由来:** <https://note.com/lytton_life/n/n2e93d544074c>（`kindle_shot_20260803.zip` として配布）
- **著作者:** lytton（<https://note.com/lytton_life>）
- **ライセンス:** MIT

### 利用範囲

本プロジェクトは kindle_shot の**設計と、テキスト処理層の一部**を参照・派生元としています。
派生したファイルには冒頭に由来を明記します。

参照・派生を予定しているモジュール:

| モジュール | 内容 |
|---|---|
| `core/text_reflow.py` | OCR 後の改行結合・段落自動整形 |
| `core/text_cleanup.py` | 行内クリーニング（句読点直後の余分な空白除去） |
| `core/text_patterns.py` | テキスト処理共通の文字クラス・正規表現 |
| `core/chapter_detector.py` | 章見出しの自動検出 |
| `core/markdown_writer.py` | Markdown 出力（NotebookLM 最適化 / ページ忠実） |
| `core/pdf_builder.py` | 検索可能 PDF の透明テキスト層生成 |

Win32 依存のキャプチャ層・GUI 層は流用せず、本プロジェクトで再設計します。

### ライセンス表示に関する注記

> **未解決:** 配布された ZIP には `LICENSE` ファイルが含まれておらず、README にもライセンスの
> 記述がありません。MIT である旨は `pyproject.toml` の `license = { text = "MIT" }` の 1 行と、
> 配布元 note 記事本文の記載によります。したがって **MIT が本来要求する「保持すべき著作権表示」
> （著作権者の氏名）が原配布物に存在しません。**
>
> 暫定的に上記のとおりハンドル名と配布元 URL で帰属表示を行っています。
> 著作者に対し、MIT である旨と著作権表示に用いるべき名義を確認中です。
> 回答が得られ次第、本項および派生ファイルのヘッダを更新します。

### MIT License

```
MIT License

Copyright (c) lytton (https://note.com/lytton_life)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## NDLOCR-Lite

- **URL:** <https://github.com/ndl-lab/ndlocr-lite>
- **著作者:** 国立国会図書館（National Diet Library, Japan）
- **ライセンス:** Creative Commons Attribution 4.0 International (CC BY 4.0)
  — <https://creativecommons.org/licenses/by/4.0/>

OCR エンジンとして利用します。レイアウト認識（DEIMv2）、文字列認識（PARSeq）、
読み順ソートの 3 モジュールからなり、推論は ONNX Runtime で行われます。

### 帰属表示

> 本製品は、国立国会図書館が CC BY 4.0 ライセンスで公開する NDLOCR-Lite を利用しています。
> <https://github.com/ndl-lab/ndlocr-lite>

### 注記

- 現時点では NDLOCR-Lite をセットアップ時に取得する構成を想定しており、本リポジトリには
  同梱しません。**ONNX モデルファイル（約 150MB）を同梱する構成に変更する場合は、
  CC BY 4.0 の帰属表示義務が配布物にも及びます。**
- NDLOCR-Lite 自身の依存ライブラリのライセンスは、同リポジトリの `LICENCE_DEPENDENCIES` を
  参照してください。
