//! ページの位置表示と、取得したページ画像の素性。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 位置表示が何を数えているか（ADR-0007 実測 5）。
///
/// リーダーは書籍によって `33/431ページ` とも `位置9783/10167 ● 96%` とも表示する。
/// **この違いは巻末を確定できるかどうかを分ける**ため、型で区別する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    /// ページ番号。総数はページ数なので、最終ページで総数と一致する。
    #[default]
    Page,
    /// Kindle の「位置」。1 ページで 41〜55 進むため、
    /// **最終ページの位置が総数と一致するとは限らない。**
    Location,
}

/// リーダーが表示している位置。
///
/// Cloud Reader のフッター（`.text-div`）にあり、
/// これがページ遷移の**確定シグナル**になる（ADR-0004 決定 3）。
/// 書籍によっては総数が無い形式もあるため、`total` は省略可能とする。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageLabel {
    /// 現在位置。
    pub current: u32,
    /// 総数。取得できない書籍では `None`。
    pub total: Option<u32>,
    /// 何を数えている値か。既存のフィクスチャを読めるよう既定は [`LabelKind::Page`]。
    #[serde(default)]
    pub kind: LabelKind,
}

impl PageLabel {
    /// ページ番号として作る。
    #[must_use]
    pub fn new(current: u32, total: Option<u32>) -> Self {
        Self { current, total, kind: LabelKind::Page }
    }

    /// Kindle の「位置」として作る。
    #[must_use]
    pub fn at_location(current: u32, total: Option<u32>) -> Self {
        Self { current, total, kind: LabelKind::Location }
    }

    /// `33/431ページ` や `位置9783/10167 ● 96%` のような表示文字列から読み取る。
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
        let kind = if text.contains("位置") { LabelKind::Location } else { LabelKind::Page };
        Some(Self { current, total, kind })
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

    /// 総数がページ数であり、**巻末を確定できる**か。
    ///
    /// 「位置」形式の書籍では、送りが止まったのが巻末なのか故障なのかを
    /// 区別できない。その場合は打ち切って `Summary.end_confirmed` を `false` にする
    /// （ADR-0007 決定 5）。
    #[must_use]
    pub fn can_confirm_end(&self) -> bool {
        self.kind == LabelKind::Page && self.total.is_some()
    }
}

impl fmt::Display for PageLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let unit = match self.kind {
            LabelKind::Page => "ページ",
            LabelKind::Location => "位置",
        };
        match (self.kind, self.total) {
            (LabelKind::Page, Some(t)) => write!(f, "{}/{}{unit}", self.current, t),
            (LabelKind::Page, None) => write!(f, "{}{unit}", self.current),
            (LabelKind::Location, Some(t)) => write!(f, "{unit}{}/{}", self.current, t),
            (LabelKind::Location, None) => write!(f, "{unit}{}", self.current),
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

    /// ADR-0007 実測 5: ページ番号を持たない書籍は「位置」で表示する。
    #[test]
    fn parses_the_location_style_footer() {
        let l = PageLabel::parse("位置9783/10167\u{2002}●\u{2002}96%").unwrap();
        assert_eq!(l, PageLabel::at_location(9783, Some(10167)));
        assert_eq!(l.to_string(), "位置9783/10167");
    }

    /// 「位置」形式では巻末を確定できない。ここを取り違えると、
    /// 全ページ撮り終えた書籍を「送りが壊れた」と誤判定する。
    #[test]
    fn only_page_numbers_can_confirm_the_end_of_a_book() {
        assert!(PageLabel::new(33, Some(431)).can_confirm_end());
        assert!(!PageLabel::at_location(9783, Some(10167)).can_confirm_end());
        assert!(!PageLabel::new(33, None).can_confirm_end());
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

    /// 種別を持たない古いフィクスチャは「ページ」として読める。
    #[test]
    fn older_fixtures_without_a_kind_still_load() {
        let l: PageLabel = serde_json::from_str(r#"{"current":33,"total":431}"#).unwrap();
        assert_eq!(l, PageLabel::new(33, Some(431)));
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
