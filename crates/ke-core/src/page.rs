//! ページの位置表示と、取得したページ画像の素性。

use std::fmt;

use serde::{Deserialize, Serialize};

/// リーダーが表示している位置。
///
/// Cloud Reader のフッターには `33/431ページ` のような表示があり、
/// これがページ遷移の**確定シグナル**になる（ADR-0004 決定 3）。
/// 書籍によっては総数が無い形式もあるため、`total` は省略可能とする。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageLabel {
    /// 現在位置。
    pub current: u32,
    /// 総ページ数。取得できない書籍では `None`。
    pub total: Option<u32>,
}

impl PageLabel {
    /// 位置と総数から作る。
    #[must_use]
    pub fn new(current: u32, total: Option<u32>) -> Self {
        Self { current, total }
    }

    /// `33/431ページ` のような表示文字列から読み取る。
    ///
    /// 桁区切りのカンマ、全角スペース、前後の余分な文字を許容する。
    /// 読み取れなければ `None`。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        // 桁区切りは先に落とす。空白に置き換えると数字が分断されてしまう。
        let digits: String = text
            .chars()
            .filter(|c| *c != ',' && *c != '，')
            .map(|c| if c.is_ascii_digit() || c == '/' { c } else { ' ' })
            .collect();
        let token = digits.split_whitespace().find(|t| t.contains('/'))?;
        let (a, b) = token.split_once('/')?;
        let current = a.parse().ok()?;
        let total = b.parse().ok();
        Some(Self { current, total })
    }

    /// 先頭ページに到達しているか。
    #[must_use]
    pub fn is_first(&self) -> bool {
        self.current <= 1
    }

    /// 末尾ページに到達しているか。総数が不明なら判定できないので `false`。
    #[must_use]
    pub fn is_last(&self) -> bool {
        self.total.is_some_and(|t| self.current >= t)
    }
}

impl fmt::Display for PageLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.total {
            Some(t) => write!(f, "{}/{}ページ", self.current, t),
            None => write!(f, "{}ページ", self.current),
        }
    }
}

/// 取得したページ画像の素性。
///
/// Cloud Reader はページをサーバ側でレンダリング済みの画像として配信するため
/// （ADR-0004）、表示サイズではなく**原寸**が OCR 入力の解像度になる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageImageInfo {
    /// 画像の出所を表す識別子（実装上は `<img>` の blob URL）。
    ///
    /// **ページごとに変わる**ため、位置表示を持たない書籍では
    /// これがページ遷移の確定シグナルになる。
    pub source: Option<String>,
    /// 原寸の幅（px）。
    pub natural_width: u32,
    /// 原寸の高さ（px）。
    pub natural_height: u32,
    /// 画像の読み込みが完了しているか。
    pub complete: bool,
}

impl PageImageInfo {
    /// 読み込み完了済みの画像として作る。出所は不明扱い。
    #[must_use]
    pub fn ready(natural_width: u32, natural_height: u32) -> Self {
        Self { source: None, natural_width, natural_height, complete: true }
    }

    /// 出所を付けた複製を返す。
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// OCR 入力として使える状態か。
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.complete && self.natural_width > 0 && self.natural_height > 0
    }

    /// 別のページの画像だと判断できるか。
    ///
    /// どちらかの出所が不明なときは判断できないので `false` を返す
    /// （「違うと断言できない」を「同じ」に倒す。誤って撮り直すより安全）。
    #[must_use]
    pub fn differs_from(&self, other: &Self) -> bool {
        match (&self.source, &other.source) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }
}

/// 1 ページを実測して得た品質指標。
///
/// フォントサイズ校正（ADR-0005 決定 2）は、設定値を読み取れないため
/// 「設定 → 実測 → 検証」で収束させる。その実測値を表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageMetrics {
    /// 1 文字あたりの画素数（縦書きなら本文行の幅の中央値）。
    pub px_per_char: u32,
    /// このページから読めた文字数。
    pub chars: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_reader_footer() {
        let l = PageLabel::parse("33/431ページ ● 6%").unwrap();
        assert_eq!(l, PageLabel::new(33, Some(431)));
        assert_eq!(l.to_string(), "33/431ページ");
    }

    #[test]
    fn parses_labels_with_thousands_separators() {
        let l = PageLabel::parse("1,234/5,678ページ").unwrap();
        assert_eq!(l, PageLabel::new(1234, Some(5678)));
    }

    #[test]
    fn returns_none_for_text_without_a_position() {
        assert!(PageLabel::parse("読書速度を学習中...").is_none());
        assert!(PageLabel::parse("").is_none());
    }

    #[test]
    fn detects_first_and_last_pages() {
        assert!(PageLabel::new(1, Some(431)).is_first());
        assert!(!PageLabel::new(2, Some(431)).is_first());
        assert!(PageLabel::new(431, Some(431)).is_last());
        assert!(!PageLabel::new(430, Some(431)).is_last());
        // 総数が不明なら末尾判定はできない
        assert!(!PageLabel::new(430, None).is_last());
    }

    #[test]
    fn an_incomplete_image_is_not_usable() {
        assert!(PageImageInfo::ready(1501, 1692).is_usable());
        assert!(!PageImageInfo { complete: false, ..PageImageInfo::ready(1501, 1692) }.is_usable());
        assert!(!PageImageInfo::ready(0, 1692).is_usable());
    }

    #[test]
    fn detects_a_different_page_by_its_blob_url() {
        let a = PageImageInfo::ready(1501, 1692).with_source("blob:https://read.amazon.co.jp/aaa");
        let b = PageImageInfo::ready(1501, 1692).with_source("blob:https://read.amazon.co.jp/bbb");
        assert!(a.differs_from(&b));
        assert!(!a.differs_from(&a.clone()));
    }

    /// 出所が分からないときは「違う」と言い切らない。
    #[test]
    fn cannot_tell_pages_apart_without_a_source() {
        let known = PageImageInfo::ready(1501, 1692).with_source("blob:x");
        let unknown = PageImageInfo::ready(1501, 1692);
        assert!(!known.differs_from(&unknown));
        assert!(!unknown.differs_from(&known));
    }
}
