//! 撮影層を、実機を一切使わずに端から端まで通す。
//!
//! 派生元の `kindle_shot` が「Win32 実機依存のため対象外」として
//! テストを 1 件も持たなかったのがこの層である（ADR-0001 §6a）。
//! `ke-nav`（判断）と `ke-cdp`（実行）を噛み合わせて 1 冊分を回し切る。
//!
//! ここで書いているループは、そのまま `ke-workflow` が実装すべき形でもある。
//! **画素/文字の実測だけは OCR を持つ層の仕事**なので、
//! 観測に後から差し込む形になっている点に注意すること。

// clippy.toml の allow-*-in-tests は #[cfg(test)] モジュールにしか効かない。
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use ke_cdp::{Browser, Direction, Effect, FakeBrowser, PageImage, apply};
use ke_core::{Action, Asin, BookSpec, DisplayTarget, Failure, PageLabel, PageMetrics, Theme};
use ke_nav::{Limits, Navigator};

const MAX_STEPS: usize = 4_000;

/// フォント段から画素/文字を返す（ADR-0005 実測 3 を段番号に読み替えたもの）。
/// 本来は OCR が測る値なので、ここでは対応表で代用する。
fn measure(font_index: u8) -> PageMetrics {
    let px_per_char = match font_index {
        0..=6 => 27,
        7..=10 => 45,
        _ => 51,
    };
    PageMetrics { px_per_char, chars: 1_796 / px_per_char * 10 }
}

fn spec() -> BookSpec {
    BookSpec::new(Asin::new("B0TESTBOOK").expect("固定の ASIN"), "テスト本")
}

fn quick_limits() -> Limits {
    Limits { book_load_timeout_ms: 5_000, page_turn_timeout_ms: 2_000, ..Limits::default() }
}

/// `ke-nav` と [`Browser`] を噛み合わせて 1 冊を回す。
struct Session {
    nav: Navigator,
    browser: FakeBrowser,
    /// `MeasurePage` の結果。次の観測に差し込む。
    pending: Option<PageMetrics>,
    captured: Vec<(PageLabel, PageImage)>,
    log: Vec<Action>,
}

impl Session {
    fn new(spec: BookSpec, browser: FakeBrowser) -> Self {
        Self {
            nav: Navigator::with_limits(spec, quick_limits()),
            browser,
            pending: None,
            captured: Vec::new(),
            log: Vec::new(),
        }
    }

    fn run(&mut self) -> Action {
        for _ in 0..MAX_STEPS {
            let mut obs = self.browser.observe().expect("模擬の観測は失敗しない");
            obs.metrics = self.pending.take();

            let action = self.nav.step(&obs);
            self.log.push(action.clone());

            match apply(&mut self.browser, &action).expect("模擬の実行は失敗しない") {
                Effect::Terminal => return action,
                Effect::ToMeasure(_) => self.pending = Some(measure(self.browser.font_index())),
                Effect::Captured { label, image } => self.captured.push((label, image)),
                Effect::Nothing => {}
            }
        }
        panic!("{MAX_STEPS} 手を超えても終端に達しなかった");
    }
}

// ------------------------------------------------------------------
// 1 冊分を通す
// ------------------------------------------------------------------

#[test]
fn captures_a_whole_book_without_touching_a_real_browser() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(20).starting_at(7));

    let last = s.run();

    assert!(matches!(last, Action::Done(_)), "{last:?}");
    assert_eq!(s.captured.len(), 20, "先頭から末尾まで撮る");
    assert_eq!(s.captured.first().map(|(l, _)| l.clone()), Some(PageLabel::new(1, Some(20))));
    assert_eq!(s.captured.last().map(|(l, _)| l.clone()), Some(PageLabel::new(20, Some(20))));

    // 同じページを二度撮っていない（画像の中身で確かめる）
    let mut pngs: Vec<&[u8]> = s.captured.iter().map(|(_, i)| i.png.as_slice()).collect();
    pngs.sort_unstable();
    pngs.dedup();
    assert_eq!(pngs.len(), 20, "取り出した画像が重複している");
}

/// 記録・再生ハーネス（ADR-0001 §6b）の土台。実機で事故が起きたときに、
/// そのセッションの行動列をそのまま回帰テストにできる必要がある。
#[test]
fn the_whole_session_can_be_saved_as_a_fixture() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(5).using_locations());
    s.run();

    let json = serde_json::to_string(&s.log).expect("行動列は直列化できる");
    let back: Vec<Action> = serde_json::from_str(&json).expect("復元できる");
    assert_eq!(back, s.log);
    assert!(s.log.len() > 10, "1 冊分の行動列になっている: {}", s.log.len());
}

#[test]
fn opens_the_book_before_anything_else() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(3));
    s.run();
    assert_eq!(s.browser.opened_url(), Some("https://read.amazon.co.jp/?asin=B0TESTBOOK"));
}

/// 白地黒字にしてから撮る。これができないと OCR 前に反転が要る（ADR-0005 実測 6）。
#[test]
fn settles_the_display_settings_before_capturing() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(3).with_theme(Theme::Dark));

    s.run();

    assert_eq!(s.browser.theme(), Theme::White);
    assert!(
        DisplayTarget::balanced().is_satisfied_by(measure(s.browser.font_index()).px_per_char),
        "フォント段 {} は目標に届いていない",
        s.browser.font_index()
    );
    // 設定を終えたらメニューは閉じている（開いたままだと本文が隠れる）
    assert!(!s.browser.observe().unwrap().settings_menu_open);
}

/// ルビ優先の目標なら、より大きい段まで上げる。
#[test]
fn uses_a_larger_font_for_the_ruby_preset() {
    let mut spec = spec();
    spec.display = DisplayTarget::ruby_first();
    let mut s = Session::new(spec, FakeBrowser::with_pages(3));

    s.run();

    assert_eq!(s.nav.px_per_char(), Some(51));
    assert!(s.browser.font_index() >= 11, "段 {}", s.browser.font_index());
}

/// 段が足りないリーダーでも、存在しない段を要求せずに撮影へ進む。
#[test]
fn copes_with_a_reader_that_has_fewer_font_steps() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(3).with_font(1, 3));

    let last = s.run();

    assert!(matches!(last, Action::Done(_)), "{last:?}");
    assert!(s.browser.font_index() <= 3);
}

// ------------------------------------------------------------------
// 実機で観測した書籍の癖
// ------------------------------------------------------------------

/// 巻末では送りの操作子が消える（ADR-0007 実測 11）。位置表示の形式に依存せず
/// 巻末を確定できるので、「位置」表示の書籍でも end_confirmed は真になる。
#[test]
fn confirms_the_end_from_the_control_that_disappears() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(12).using_locations());

    let last = s.run();

    let Action::Done(summary) = last else { panic!("失敗にしてはいけない: {last:?}") };
    assert!(summary.end_confirmed, "操作子が消えたのだから巻末と確定できる");
    assert_eq!(s.captured.len(), 12);
}

/// 操作子が消えないまま送りが止まった場合は、巻末なのか故障なのか分からない。
/// 「位置」表示の書籍では区別できないので、確定せずに終える。
#[test]
fn finishes_without_confirming_when_the_end_cannot_be_observed() {
    let browser = FakeBrowser::with_pages(30).using_locations().reachable_pages(1, 12);
    let mut s = Session::new(spec(), browser);

    let last = s.run();

    let Action::Done(summary) = last else { panic!("失敗にしてはいけない: {last:?}") };
    assert!(!summary.end_confirmed, "確定できないものを確定したと言ってはいけない");
    assert_eq!(s.captured.len(), 12);
}

/// 位置表示を持たない書籍では、blob URL の変化でページ送りを確定させる。
#[test]
fn captures_a_book_without_any_position_display() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(6).without_labels());

    let last = s.run();

    assert!(matches!(last, Action::Done(_)), "{last:?}");
    assert_eq!(s.captured.len(), 6);
    assert_eq!(s.captured.first().map(|(l, _)| l.clone()), Some(PageLabel::new(1, None)));
}

/// 先頭まで戻れない書籍でも、止まったところから撮り始める。
#[test]
fn starts_capturing_where_the_rewind_stops() {
    let mut s =
        Session::new(spec(), FakeBrowser::with_pages(30).starting_at(20).reachable_pages(5, 30));

    let last = s.run();

    assert!(matches!(last, Action::Done(_)), "{last:?}");
    assert_eq!(s.captured.first().map(|(l, _)| l.clone()), Some(PageLabel::new(5, Some(30))));
}

#[test]
fn fails_when_the_settings_menu_never_opens() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(5).with_broken_menu());
    assert_eq!(s.run(), Action::Fail(Failure::SettingsMenuUnavailable));
}

#[test]
fn fails_when_the_book_never_renders() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(5).ready_after(u32::MAX));
    let last = s.run();
    assert!(matches!(last, Action::Fail(Failure::BookDidNotLoad { .. })), "{last:?}");
    assert!(s.captured.is_empty());
}

#[test]
fn fails_when_pages_stop_advancing_before_the_end() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(50).that_never_turns());
    let last = s.run();
    assert!(matches!(last, Action::Fail(Failure::PageDidNotAdvance { .. })), "{last:?}");
}

// ------------------------------------------------------------------
// 行動 → 原始操作の翻訳
// ------------------------------------------------------------------

/// ページ画像が出てくるまで観測を進めた模擬を返す。
fn ready_browser() -> FakeBrowser {
    let mut b = FakeBrowser::with_pages(10).starting_at(5).with_theme(Theme::Dark);
    b.observe().expect("模擬の観測は失敗しない");
    b
}

#[test]
fn translates_opening_and_page_turns() {
    let mut b = ready_browser();

    let effect = apply(&mut b, &Action::OpenBook { url: "https://x/".into() }).unwrap();
    assert_eq!(effect, Effect::Nothing);
    assert_eq!(b.opened_url(), Some("https://x/"));

    apply(&mut b, &Action::PressNext).unwrap();
    assert_eq!(b.page(), 6);
    // 実機の送りには最小間隔があるので、模擬でも間を置かないと無視される。
    b.sleep(200);
    apply(&mut b, &Action::PressPrev).unwrap();
    assert_eq!(b.page(), 5);
}

/// 間を置かずに送ると無視される（ADR-0007 実測 9）。
/// これを再現しないと、押し直しの要らない世界でテストしてしまう。
#[test]
fn a_page_turn_sent_too_soon_is_ignored() {
    let mut b = ready_browser();
    b.sleep(500);

    b.turn_page(Direction::Next).unwrap();
    assert_eq!(b.page(), 6);

    b.turn_page(Direction::Next).unwrap();
    assert_eq!(b.page(), 6, "間隔が足りない送りは無視される");
    assert_eq!(b.swallowed_turns(), 1);

    b.sleep(200);
    b.turn_page(Direction::Next).unwrap();
    assert_eq!(b.page(), 7, "間を置けば通る");
}

#[test]
fn translates_display_settings() {
    let mut b = ready_browser();

    apply(&mut b, &Action::OpenSettingsMenu).unwrap();
    assert!(b.observe().unwrap().settings_menu_open);

    apply(&mut b, &Action::SetTheme(Theme::White)).unwrap();
    assert_eq!(b.theme(), Theme::White);

    apply(&mut b, &Action::SetFontSize { index: 9 }).unwrap();
    assert_eq!(b.font_index(), 9);

    apply(&mut b, &Action::CloseSettingsMenu).unwrap();
    assert!(!b.observe().unwrap().settings_menu_open);
}

/// 撮影と実測はどちらもページ画像を取り出すが、**用途が違う**。
/// 取り違えると、実測用の 1 枚が本文として保存されてしまう。
#[test]
fn tells_capturing_and_measuring_apart() {
    let mut b = ready_browser();
    let label = PageLabel::new(5, Some(10));

    let captured = apply(&mut b, &Action::CapturePage { label: label.clone() }).unwrap();
    assert!(matches!(captured, Effect::Captured { label: l, .. } if l == label));
    assert!(matches!(apply(&mut b, &Action::MeasurePage).unwrap(), Effect::ToMeasure(_)));
}

#[test]
fn treats_the_terminal_actions_as_nothing_to_do() {
    let mut b = ready_browser();
    let summary =
        ke_core::Summary { captured_pages: 1, px_per_char: Some(45), end_confirmed: true };
    assert_eq!(apply(&mut b, &Action::Done(summary)).unwrap(), Effect::Terminal);
    assert_eq!(
        apply(&mut b, &Action::Fail(Failure::SettingsMenuUnavailable)).unwrap(),
        Effect::Terminal
    );
}

/// `Wait` は実際に眠らせる操作なので、模擬では観測の経過時間として現れる。
#[test]
fn a_wait_shows_up_as_elapsed_time_on_the_next_observation() {
    let mut b = FakeBrowser::with_pages(2);
    apply(&mut b, &Action::Wait { ms: 1_500, reason: ke_core::WaitReason::PageTurn }).unwrap();
    assert!(b.observe().unwrap().elapsed_ms >= 1_500);
}

/// 設定メニューが閉じているのに設定を変えようとしたら失敗する。
/// 実機がそうなので、模擬が黙って成功すると順序の誤りを見逃す。
#[test]
fn changing_settings_requires_an_open_menu() {
    let mut b = FakeBrowser::with_pages(2);
    assert!(b.set_theme(Theme::White).is_err());
    assert!(b.set_font_size(9).is_err());
}

#[test]
fn a_page_image_is_not_available_before_the_book_renders() {
    let mut b = FakeBrowser::with_pages(2).ready_after(2);
    assert!(b.capture_page().is_err());
    b.observe().unwrap();
    b.observe().unwrap();
    b.observe().unwrap();
    assert!(b.capture_page().is_ok());
}

/// 送り・戻しは読み進む向きであって画面の左右ではない。
#[test]
fn turning_moves_along_the_reading_direction() {
    let mut b = FakeBrowser::with_pages(10).starting_at(1);
    b.turn_page(Direction::Prev).unwrap();
    assert_eq!(b.page(), 1, "先頭より前には行かない");
    b.sleep(200);
    b.turn_page(Direction::Next).unwrap();
    assert_eq!(b.page(), 2);
}

/// 空振りしても最後まで撮り切る。押し直さないと、「位置」表示の書籍では
/// 本の途中で静かに打ち切ってしまう。
#[test]
fn captures_everything_even_though_some_turns_are_swallowed() {
    let mut s = Session::new(spec(), FakeBrowser::with_pages(15).using_locations());

    let last = s.run();

    assert!(matches!(last, Action::Done(_)), "{last:?}");
    assert_eq!(s.captured.len(), 15);
    assert!(s.browser.swallowed_turns() > 0, "空振りを再現できていない");
}
