//! ページ画像を実測して画素/文字を出す層への継ぎ目。
//!
//! 実装は Python の常駐 OCR ワーカー（`ke_ocr`）になる予定で、まだ無い。
//! trait にしてあるのは、**無いものを無いまま扱えるようにする**ためである。

use ke_core::PageMetrics;

use crate::error::{Error, Result};

/// ページ画像 1 枚から品質指標を測るもの。
///
/// フォントサイズ校正（ADR-0005 決定 2）が必要とする「1 ページ実測して
/// 目標範囲か確認する」の実測側。縦書きなら本文行の幅の中央値が画素/文字になる。
pub trait Measurer {
    /// PNG のバイト列を測る。
    fn measure(&mut self, png: &[u8]) -> Result<PageMetrics>;
}

/// OCR が無い間の仮の実測。**返す値は測定結果ではない。**
///
/// これを使って走らせた結果の `Summary::px_per_char` は、
/// 「そう設定した」という意味しか持たない。実測値として引用してはいけない。
#[derive(Debug, Clone, Copy)]
pub struct StubMeasurer {
    px_per_char: u32,
    chars: u32,
    /// 何回目かで失敗させる（失敗経路の検証用）。
    fail_after: Option<u32>,
    calls: u32,
}

impl StubMeasurer {
    /// 常に同じ値を返す仮の実測。
    #[must_use]
    pub fn returning(px_per_char: u32) -> Self {
        Self { px_per_char, chars: 0, fail_after: None, calls: 0 }
    }

    /// 指定回数を超えたら失敗する。
    #[must_use]
    pub fn failing_after(mut self, calls: u32) -> Self {
        self.fail_after = Some(calls);
        self
    }

    /// これまでに測った回数。
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls
    }
}

impl Measurer for StubMeasurer {
    fn measure(&mut self, _png: &[u8]) -> Result<PageMetrics> {
        self.calls = self.calls.saturating_add(1);
        if self.fail_after.is_some_and(|n| self.calls > n) {
            return Err(Error::Measure("仮の実測を打ち切りました".to_owned()));
        }
        Ok(PageMetrics { px_per_char: self.px_per_char, chars: self.chars })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_returns_what_it_was_told_to() {
        let mut m = StubMeasurer::returning(45);
        assert_eq!(m.measure(b"x").unwrap().px_per_char, 45);
        assert_eq!(m.calls(), 1);
    }

    #[test]
    fn the_stub_can_be_made_to_fail() {
        let mut m = StubMeasurer::returning(45).failing_after(1);
        assert!(m.measure(b"x").is_ok());
        assert!(m.measure(b"x").is_err());
    }
}
