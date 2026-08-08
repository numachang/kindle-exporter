//! ナビゲーション状態機械が「見るもの」。
//!
//! ここに現れるのは**観測できた事実だけ**であり、解釈は一切含まない。
//! これにより状態機械を I/O から切り離し、実機ゼロでテストできる
//! （ADR-0001 §6a）。記録した観測列をそのまま回帰テストの
//! フィクスチャにできるよう、シリアライズ可能にしてある。

use serde::{Deserialize, Serialize};

use crate::{PageImageInfo, PageLabel, PageMetrics, Theme};

/// 画面上の矩形（CSS ピクセル）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// 左端。
    pub x: f64,
    /// 上端。
    pub y: f64,
    /// 幅。
    pub width: f64,
    /// 高さ。
    pub height: f64,
}

impl Rect {
    /// 矩形を作る。
    #[must_use]
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    /// 横方向に `fraction`（0.0〜1.0）だけ進んだ位置の、縦は中央の座標。
    ///
    /// フォントサイズのスライダーをクリックする座標を求めるのに使う。
    /// `fraction` は 0.0〜1.0 に丸める。NaN は 0.0 として扱う
    /// （`f32::clamp` は NaN をそのまま返すため、明示的に潰す）。
    #[must_use]
    pub fn point_at_fraction(&self, fraction: f32) -> (f64, f64) {
        let f = if fraction.is_nan() { 0.0 } else { f64::from(fraction.clamp(0.0, 1.0)) };
        (self.x + self.width * f, self.y + self.height / 2.0)
    }
}

/// ある時点でリーダーについて観測できたこと。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Observation {
    /// 直前の行動からの経過時間（ミリ秒）。待ち時間の打ち切り判定に使う。
    pub elapsed_ms: u64,
    /// フッターのページ表示。読み取れなければ `None`。
    pub page: Option<PageLabel>,
    /// 本文のページ画像。まだ無ければ `None`。
    pub image: Option<PageImageInfo>,
    /// 設定メニューが開いているか（`ion-menu` の `show-menu` で判定）。
    pub settings_menu_open: bool,
    /// フォントサイズのスライダーの位置。設定メニューが閉じていれば `None`。
    pub font_slider: Option<Rect>,
    /// 現在の配色テーマ。判定できなければ `None`。
    pub theme: Option<Theme>,
    /// 直前に `MeasurePage` を指示した場合の実測結果。
    pub metrics: Option<PageMetrics>,
}

impl Observation {
    /// 本文のページ画像が OCR 入力として使える状態か。
    #[must_use]
    pub fn has_usable_image(&self) -> bool {
        self.image.as_ref().is_some_and(PageImageInfo::is_usable)
    }

    /// 本が開いて読める状態になっているか。
    ///
    /// 判定はページ画像だけで行う。位置表示を持たない書籍があるため、
    /// 位置表示の有無を条件にしてはいけない。
    #[must_use]
    pub fn is_book_ready(&self) -> bool {
        self.has_usable_image()
    }

    /// 直前の観測と比べて、別のページに移ったと言えるか。
    ///
    /// 位置表示があればそれで判断する（ADR-0004 決定 3）。
    /// 位置表示を持たない書籍では、ページ画像の出所（blob URL）の変化で代用する。
    #[must_use]
    pub fn advanced_from(&self, previous: &Self) -> bool {
        if let (Some(now), Some(before)) = (&self.page, &previous.page) {
            return now != before;
        }
        match (&self.image, &previous.image) {
            (Some(now), Some(before)) => now.differs_from(before),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_slider_click_points() {
        // ADR-0005 で実測したスライダーの矩形
        let r = Rect::new(1455.0, 217.0, 312.0, 42.0);
        assert_eq!(r.point_at_fraction(0.0), (1455.0, 238.0));
        assert_eq!(r.point_at_fraction(1.0), (1767.0, 238.0));
        assert_eq!(r.point_at_fraction(0.5), (1611.0, 238.0));
    }

    #[test]
    fn clamps_out_of_range_fractions() {
        let r = Rect::new(0.0, 0.0, 100.0, 10.0);
        assert_eq!(r.point_at_fraction(-3.0), (0.0, 5.0));
        assert_eq!(r.point_at_fraction(9.0), (100.0, 5.0));
        assert_eq!(r.point_at_fraction(f32::NAN), (0.0, 5.0));
    }

    #[test]
    fn a_book_is_ready_once_its_page_image_is_usable() {
        let mut o = Observation::default();
        assert!(!o.is_book_ready());

        o.image = Some(PageImageInfo { complete: false, ..PageImageInfo::ready(1501, 1692) });
        assert!(!o.is_book_ready(), "読み込み途中の画像では準備できていない");

        o.image = Some(PageImageInfo::ready(1501, 1692));
        assert!(o.is_book_ready());
    }

    /// 位置表示を持たない書籍があるため、位置表示を準備完了の条件にしてはいけない。
    #[test]
    fn readiness_does_not_require_a_page_label() {
        let o = Observation {
            page: None,
            image: Some(PageImageInfo::ready(1501, 1692)),
            ..Observation::default()
        };
        assert!(o.is_book_ready());
    }

    fn at(page: Option<PageLabel>, blob: &str) -> Observation {
        Observation {
            page,
            image: Some(PageImageInfo::ready(1501, 1692).with_source(blob)),
            ..Observation::default()
        }
    }

    #[test]
    fn uses_the_page_label_to_detect_advancement() {
        let a = at(Some(PageLabel::new(33, Some(431))), "blob:same");
        let b = at(Some(PageLabel::new(34, Some(431))), "blob:same");
        assert!(b.advanced_from(&a));
        assert!(!a.advanced_from(&a.clone()));
    }

    /// 位置表示を持たない書籍では blob URL の変化で代用する。
    #[test]
    fn falls_back_to_the_image_source_without_a_page_label() {
        let a = at(None, "blob:aaa");
        let b = at(None, "blob:bbb");
        assert!(b.advanced_from(&a));
        assert!(!a.advanced_from(&at(None, "blob:aaa")));
    }

    #[test]
    fn reports_no_advancement_when_nothing_is_observable() {
        let empty = Observation::default();
        assert!(!empty.advanced_from(&Observation::default()));
    }
}
