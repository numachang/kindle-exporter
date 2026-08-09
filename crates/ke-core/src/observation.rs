//! ナビゲーション状態機械が「見るもの」。
//!
//! ここに現れるのは**観測できた事実だけ**であり、解釈は一切含まない。
//! これにより状態機械を I/O から切り離し、実機ゼロでテストできる
//! （ADR-0001 §6a）。記録した観測列をそのまま回帰テストの
//! フィクスチャにできるよう、シリアライズ可能にしてある。

use serde::{Deserialize, Serialize};

use crate::{FontControl, PageImageInfo, PageLabel, PageMetrics, Theme};

/// ページ送りの操作子が見えているか（ADR-0007 実測 11）。
///
/// **巻末では「次のページ」が、先頭では「前のページ」が DOM から消える。**
/// これは位置表示の形式にも、位置表示の有無にも依存しない**端の確定シグナル**であり、
/// 「送っても進まなくなった」という推測より確実である。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnControls {
    /// 次のページへ送れるか。
    pub next: bool,
    /// 前のページへ戻せるか。
    pub prev: bool,
}

impl TurnControls {
    /// 両方向に動ける（本の途中にいる）状態を作る。
    #[must_use]
    pub fn both() -> Self {
        Self { next: true, prev: true }
    }

    /// 巻末（次へ進めない）。
    #[must_use]
    pub fn at_end() -> Self {
        Self { next: false, prev: true }
    }

    /// 先頭（前へ戻れない）。
    #[must_use]
    pub fn at_start() -> Self {
        Self { next: true, prev: false }
    }
}

/// ある時点でリーダーについて観測できたこと。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Observation {
    /// 直前の行動からの経過時間（ミリ秒）。待ち時間の打ち切り判定に使う。
    pub elapsed_ms: u64,
    /// フッターの位置表示。読み取れなければ `None`。
    pub page: Option<PageLabel>,
    /// 本文のページ画像。まだ無ければ `None`。
    pub image: Option<PageImageInfo>,
    /// 設定メニューが開いているか（`ion-menu` の `show-menu` で判定）。
    pub settings_menu_open: bool,
    /// フォントサイズの現在段と最大段。設定メニューが閉じていれば `None`。
    pub font: Option<FontControl>,
    /// 現在の配色テーマ。判定できなければ `None`。
    pub theme: Option<Theme>,
    /// ページ送りの操作子。観測できなければ `None`（本がまだ描画されていないなど）。
    #[serde(default)]
    pub turn_controls: Option<TurnControls>,
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

    /// 表示設定を操作できる状態か（メニューが開き、操作子が見えている）。
    #[must_use]
    pub fn can_change_settings(&self) -> bool {
        self.settings_menu_open && self.font.is_some()
    }

    /// **巻末にいると確認できた**か。
    ///
    /// 送りの操作子が消えていることで判断する（ADR-0007 実測 11）。
    /// 観測できていなければ `false` を返す。「確認できない」を
    /// 「巻末ではない」に倒すので、誤って早く終わることはない。
    #[must_use]
    pub fn at_end_of_book(&self) -> bool {
        self.turn_controls.is_some_and(|c| !c.next)
    }

    /// **先頭にいると確認できた**か。
    #[must_use]
    pub fn at_start_of_book(&self) -> bool {
        self.turn_controls.is_some_and(|c| !c.prev)
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

    /// メニューが開いていても操作子が見えていなければ設定はできない
    /// （開閉アニメーションの途中で観測すると起こる）。
    #[test]
    fn settings_need_both_an_open_menu_and_a_visible_control() {
        let mut o = Observation::default();
        assert!(!o.can_change_settings());
        o.settings_menu_open = true;
        assert!(!o.can_change_settings());
        o.font = Some(FontControl::new(5, 13));
        assert!(o.can_change_settings());
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

    /// 端の判定は「消えていること」で行う。観測できていないときに
    /// 端だと言ってしまうと、本の途中で撮影を終えてしまう。
    #[test]
    fn the_ends_of_a_book_are_only_claimed_when_actually_observed() {
        let unknown = Observation::default();
        assert!(!unknown.at_end_of_book(), "観測できていないなら巻末とは言わない");
        assert!(!unknown.at_start_of_book());

        let middle = Observation { turn_controls: Some(TurnControls::both()), ..unknown.clone() };
        assert!(!middle.at_end_of_book());
        assert!(!middle.at_start_of_book());

        let end = Observation { turn_controls: Some(TurnControls::at_end()), ..unknown.clone() };
        assert!(end.at_end_of_book());
        assert!(!end.at_start_of_book());

        let start = Observation { turn_controls: Some(TurnControls::at_start()), ..unknown };
        assert!(start.at_start_of_book());
        assert!(!start.at_end_of_book());
    }

    /// 記録・再生ハーネス（ADR-0001 §6b）のため、観測は JSON で往復できる。
    #[test]
    fn round_trips_through_json() {
        let o = Observation {
            elapsed_ms: 250,
            page: Some(PageLabel::at_location(9783, Some(10167))),
            image: Some(PageImageInfo::ready(2199, 1692).with_source("blob:x")),
            settings_menu_open: true,
            font: Some(FontControl::new(5, 13)),
            theme: Some(Theme::White),
            turn_controls: Some(TurnControls::both()),
            metrics: Some(PageMetrics { px_per_char: 45, chars: 536 }),
        };
        let json = serde_json::to_string(&o).unwrap();
        assert_eq!(serde_json::from_str::<Observation>(&json).unwrap(), o);
    }
}
