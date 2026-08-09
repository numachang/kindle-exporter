//! ナビゲーション状態機械を、実機を一切使わずに端から端まで動かす。
//!
//! 派生元の `kindle_shot` はこの層にテストが 1 件も無く、
//! 「Win32 実機依存のため対象外」と明記されていた。
//! 状態機械から I/O を追い出したことで、ここが全部テストできるようになる
//! （ADR-0001 §6a）。

// clippy.toml の allow-*-in-tests は #[cfg(test)] モジュールにしか効かない。
// tests/ 配下は独立した crate なので、ここで明示的に許可する。
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use ke_core::{
    Action, Asin, BookSpec, DisplayTarget, Failure, FontControl, Observation, PageImageInfo,
    PageLabel, PageMetrics, Theme,
};
use ke_nav::{Limits, Navigator};

/// テストで無限ループにならないための保険。
const MAX_STEPS: usize = 4_000;

/// 実機のフォント段数（ADR-0007 実測 2 で 0〜13 の 14 段）。
const MAX_FONT_INDEX: u8 = 13;

/// フォント段から画素/文字を返す関数（ADR-0005 実測 3 を段番号に読み替えた応答）。
type FontResponse = fn(u8) -> u32;

fn measured_from_index(index: u8) -> u32 {
    match index {
        0..=6 => 27,
        7..=10 => 45,
        _ => 51,
    }
}

/// かなり上げないと目標に届かないリーダー（再校正が要る場合の検証用）。
fn stiff_font(index: u8) -> u32 {
    if index < 12 { 27 } else { 45 }
}

/// 常に目標に届かないリーダー（校正が収束しない場合の検証用）。
fn always_tiny(_index: u8) -> u32 {
    12
}

/// リーダーの模擬。行動を受けて内部状態を動かし、次の観測を作る。
struct FakeReader {
    page: u32,
    total: Option<u32>,
    /// 読み込み完了までに必要な観測回数。
    load_ticks: u32,
    ticks: u32,
    menu_open: bool,
    /// メニューが開けるか（開けない障害の再現用）。
    menu_operable: bool,
    theme: Option<Theme>,
    font_index: u8,
    font_max: u8,
    font_response: FontResponse,
    /// 次の観測に実測値を載せるか。
    emit_metrics: bool,
    /// ページ送りを受け付けるか（進まなくなる障害の再現用）。
    can_advance: bool,
    /// 巻き戻しで到達できる最小ページ（1 に戻れない書籍の再現用）。
    min_page: u32,
    /// 送りで到達できる最大位置。`None` なら総数まで。
    stop_at: Option<u32>,
    /// 位置表示を持つか。
    has_label: bool,
    /// 位置表示が「ページ」ではなく Kindle の「位置」か（ADR-0007 実測 5）。
    locations: bool,
    /// 表示設定を変えたあと、ページ画像が消えている観測回数（ADR-0007 実測 8）。
    rerender_ticks: u32,
    rerender_left: u32,
    /// ページ送りを 1 回おきに無視する（ADR-0007 実測 9 の最悪ケース）。
    swallows_turns: bool,
    swallowed: bool,
    captured: Vec<PageLabel>,
}

impl FakeReader {
    fn new(page: u32, total: u32) -> Self {
        Self {
            page,
            total: Some(total),
            load_ticks: 0,
            ticks: 0,
            menu_open: false,
            menu_operable: true,
            theme: Some(Theme::Dark),
            font_index: 5, // 実機の既定値（ADR-0007 実測 2）
            font_max: MAX_FONT_INDEX,
            font_response: measured_from_index,
            emit_metrics: false,
            can_advance: true,
            min_page: 1,
            stop_at: None,
            has_label: true,
            locations: false,
            rerender_ticks: 0,
            rerender_left: 0,
            swallows_turns: false,
            swallowed: false,
            captured: Vec::new(),
        }
    }

    fn label(&self) -> PageLabel {
        if self.locations {
            PageLabel::at_location(self.page, self.total)
        } else {
            PageLabel::new(self.page, self.total)
        }
    }

    fn observe(&mut self) -> Observation {
        self.ticks += 1;
        // 表示設定を変えるとページが作り直され、その間は画像も位置表示も消える。
        let rendering = self.rerender_left > 0;
        self.rerender_left = self.rerender_left.saturating_sub(1);
        let loaded = self.ticks > self.load_ticks && !rendering;
        Observation {
            elapsed_ms: 1_000,
            page: (loaded && self.has_label).then(|| self.label()),
            image: loaded.then(|| {
                PageImageInfo::ready(1501, 1692).with_source(format!("blob:page-{}", self.page))
            }),
            settings_menu_open: self.menu_open,
            font: self.menu_open.then(|| FontControl::new(self.font_index, self.font_max)),
            theme: self.theme,
            metrics: self.take_metrics(),
        }
    }

    fn take_metrics(&mut self) -> Option<PageMetrics> {
        if !std::mem::take(&mut self.emit_metrics) {
            return None;
        }
        Some(PageMetrics { px_per_char: (self.font_response)(self.font_index), chars: 536 })
    }

    fn apply(&mut self, action: &Action) {
        match action {
            Action::OpenSettingsMenu if self.menu_operable => self.menu_open = true,
            Action::CloseSettingsMenu if self.menu_operable => self.menu_open = false,
            Action::SetTheme(t) if self.theme != Some(*t) => {
                self.theme = Some(*t);
                self.rerender_left = self.rerender_ticks;
            }
            Action::SetFontSize { index } if self.font_index != (*index).min(self.font_max) => {
                self.font_index = (*index).min(self.font_max);
                self.rerender_left = self.rerender_ticks;
            }
            Action::MeasurePage => self.emit_metrics = true,
            Action::CapturePage { label } => self.captured.push(label.clone()),
            Action::PressNext => self.turn(1),
            Action::PressPrev => self.turn(-1),
            _ => {}
        }
    }

    fn turn(&mut self, delta: i64) {
        if !self.can_advance {
            return;
        }
        if self.swallows_turns {
            self.swallowed = !self.swallowed;
            if self.swallowed {
                return;
            }
        }
        let next = i64::from(self.page) + delta;
        let hi = self.stop_at.or(self.total).map_or(i64::MAX, i64::from);
        self.page = next.clamp(i64::from(self.min_page), hi).try_into().unwrap_or(self.page);
    }
}

/// 状態機械とリーダーを終端まで回し、最後の行動と全行動列を返す。
fn drive(nav: &mut Navigator, reader: &mut FakeReader) -> (Action, Vec<Action>) {
    let mut log = Vec::new();
    for _ in 0..MAX_STEPS {
        let obs = reader.observe();
        let action = nav.step(&obs);
        log.push(action.clone());
        if action.is_terminal() {
            return (action, log);
        }
        reader.apply(&action);
    }
    panic!("{MAX_STEPS} 手を超えても終端に達しなかった");
}

fn spec() -> BookSpec {
    BookSpec::new(Asin::new("B0TESTBOOK").expect("固定の ASIN"), "テスト本")
}

fn quick_limits() -> Limits {
    Limits { book_load_timeout_ms: 5_000, page_turn_timeout_ms: 2_000, ..Limits::default() }
}

fn count<F: Fn(&Action) -> bool>(log: &[Action], f: F) -> usize {
    log.iter().filter(|a| f(a)).count()
}

// ------------------------------------------------------------------
// 正常系
// ------------------------------------------------------------------

#[test]
fn captures_a_whole_book_from_the_middle_of_it() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    // 33/431 ページ目から始める（実機で観測した状況）
    let mut reader = FakeReader::new(33, 431);

    let (last, log) = drive(&mut nav, &mut reader);

    assert_eq!(
        last,
        Action::Done(ke_core::Summary {
            captured_pages: 431,
            px_per_char: Some(45),
            end_confirmed: true
        })
    );
    assert_eq!(reader.captured.len(), 431, "先頭から末尾まで撮る");
    assert_eq!(reader.captured.first(), Some(&PageLabel::new(1, Some(431))));
    assert_eq!(reader.captured.last(), Some(&PageLabel::new(431, Some(431))));
    assert_eq!(count(&log, |a| matches!(a, Action::CapturePage { .. })), 431);
    // 同じページを二度撮っていない
    let mut seen = reader.captured.clone();
    seen.dedup();
    assert_eq!(seen.len(), 431);
}

#[test]
fn opens_the_book_by_its_reader_url_first() {
    let mut nav = Navigator::new(spec());
    let obs = Observation::default();
    assert_eq!(
        nav.step(&obs),
        Action::OpenBook { url: "https://read.amazon.co.jp/?asin=B0TESTBOOK".into() }
    );
}

#[test]
fn sets_the_theme_before_touching_the_font_slider() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 3);
    let (_, log) = drive(&mut nav, &mut reader);

    let theme_at = log.iter().position(|a| matches!(a, Action::SetTheme(_)));
    let font_at = log.iter().position(|a| matches!(a, Action::SetFontSize { .. }));
    assert!(theme_at.is_some(), "暗色のままなら明色に変える");
    assert!(theme_at < font_at, "テーマを決めてからフォントを触る");
    assert_eq!(reader.theme, Some(Theme::White), "白地黒字にして反転処理を不要にする");
}

#[test]
fn skips_the_theme_step_when_it_is_already_correct() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 2);
    reader.theme = Some(Theme::White);
    let (_, log) = drive(&mut nav, &mut reader);
    assert_eq!(count(&log, |a| matches!(a, Action::SetTheme(_))), 0);
}

#[test]
fn recalibrates_the_font_until_the_target_is_met() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 2);
    // 初手（中央の段）では届かないので、上げ直す必要がある
    reader.font_response = stiff_font;

    let (_, log) = drive(&mut nav, &mut reader);

    let steps: Vec<u8> = log
        .iter()
        .filter_map(|a| match a {
            Action::SetFontSize { index } => Some(*index),
            _ => None,
        })
        .collect();
    assert!(steps.len() >= 2, "1 回で決まらないなら再校正する: {steps:?}");
    assert!(steps.windows(2).all(|w| w[1] > w[0]), "小さすぎるので段を上げ続ける: {steps:?}");
    assert_eq!(nav.px_per_char(), Some(45), "目標範囲に収束する");
}

/// 段はリーダーが持つ最大段を超えない。
#[test]
fn never_asks_for_a_font_step_the_reader_does_not_have() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 2);
    reader.font_max = 4;
    reader.font_response = always_tiny; // 目標に届かないので段を上げ切る

    let (_, log) = drive(&mut nav, &mut reader);

    for a in &log {
        if let Action::SetFontSize { index } = a {
            assert!(*index <= 4, "存在しない段 {index} を指定した");
        }
    }
}

#[test]
fn accepts_a_larger_font_when_the_ruby_preset_is_requested() {
    let mut s = spec();
    s.display = DisplayTarget::ruby_first();
    let mut nav = Navigator::with_limits(s, quick_limits());
    let mut reader = FakeReader::new(1, 2);
    drive(&mut nav, &mut reader);
    assert_eq!(nav.px_per_char(), Some(51));
}

#[test]
fn rewinds_to_the_first_page_before_capturing() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(50, 60);
    let (_, log) = drive(&mut nav, &mut reader);

    let first_capture = log.iter().position(|a| matches!(a, Action::CapturePage { .. }));
    let prev_count = count(&log, |a| matches!(a, Action::PressPrev));
    assert!(prev_count >= 49, "50 ページ目から先頭まで戻る: {prev_count}");
    assert!(first_capture.is_some());
    assert_eq!(reader.captured.first(), Some(&PageLabel::new(1, Some(60))));
}

#[test]
fn does_not_rewind_when_already_on_the_first_page() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 3);
    let (_, log) = drive(&mut nav, &mut reader);
    assert_eq!(count(&log, |a| matches!(a, Action::PressPrev)), 0);
}

/// 先頭に戻れない書籍でも、位置が動かなくなったら撮影に移る。
#[test]
fn treats_a_stalled_rewind_as_the_start_of_the_book() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(20, 30);
    reader.min_page = 5; // 5 ページ目より前には戻れない

    let (last, _) = drive(&mut nav, &mut reader);

    assert!(matches!(last, Action::Done(_)), "止まっても失敗にはしない: {last:?}");
    assert_eq!(reader.captured.first(), Some(&PageLabel::new(5, Some(30))));
}

/// 位置表示を持たない書籍は、ページ画像の blob URL の変化で送りを判定する。
#[test]
fn captures_books_without_a_page_label() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 4);
    reader.has_label = false;

    let (last, _) = drive(&mut nav, &mut reader);

    assert!(matches!(last, Action::Done(_)), "{last:?}");
    assert!(reader.captured.len() >= 4, "撮れた枚数: {}", reader.captured.len());
    // 位置表示が無いので 1 起点の連番で代用する
    assert_eq!(reader.captured.first(), Some(&PageLabel::new(1, None)));
}

// ------------------------------------------------------------------
// 異常系
// ------------------------------------------------------------------

#[test]
fn fails_when_the_book_never_loads() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 10);
    reader.load_ticks = u32::MAX; // いつまでも描画されない

    let (last, log) = drive(&mut nav, &mut reader);

    assert!(
        matches!(last, Action::Fail(Failure::BookDidNotLoad { waited_ms }) if waited_ms >= 5_000),
        "{last:?}"
    );
    assert_eq!(count(&log, |a| matches!(a, Action::CapturePage { .. })), 0);
}

/// ページ送りは一定の割合で空振りする（ADR-0007 実測 9）。押し直さないと、
/// 「位置」表示の書籍では巻末と誤認して本の途中で静かに打ち切ってしまう。
#[test]
fn presses_again_when_a_page_turn_is_swallowed() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 12);
    reader.swallows_turns = true;

    let (last, log) = drive(&mut nav, &mut reader);

    assert!(matches!(last, Action::Done(_)), "{last:?}");
    assert_eq!(reader.captured.len(), 12, "空振りしても全ページ撮り切る");
    assert!(
        count(&log, |a| matches!(a, Action::PressNext)) > 12,
        "空振りしたぶん押し直しているはず"
    );
}

/// 空振りが続いても「先頭に着いた」と誤認して本の途中から撮り始めない。
#[test]
fn a_swallowed_press_is_not_mistaken_for_the_start_of_the_book() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(8, 12);
    reader.swallows_turns = true;

    drive(&mut nav, &mut reader);

    assert_eq!(reader.captured.first(), Some(&PageLabel::new(1, Some(12))), "先頭から撮る");
}

/// 表示設定を変えるとページが作り直され、しばらくページ画像が消える
/// （ADR-0007 実測 8 で 2.8 秒）。戻る前に実測しようとすると実機で落ちる。
#[test]
fn waits_for_the_page_to_be_rebuilt_after_changing_settings() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 4);
    reader.rerender_ticks = 4;

    let (last, log) = drive(&mut nav, &mut reader);

    assert!(matches!(last, Action::Done(_)), "{last:?}");
    assert_eq!(reader.captured.len(), 4);
    // 画像が戻る前に測ろうとしていない
    let measure_at = log.iter().position(|a| matches!(a, Action::MeasurePage));
    let font_at = log.iter().position(|a| matches!(a, Action::SetFontSize { .. }));
    assert!(measure_at > font_at, "設定より前に測っている: {measure_at:?} {font_at:?}");
}

#[test]
fn fails_when_the_page_never_comes_back_after_a_settings_change() {
    let limits = Limits { render_timeout_ms: 3_000, ..quick_limits() };
    let mut nav = Navigator::with_limits(spec(), limits);
    let mut reader = FakeReader::new(1, 4);
    reader.rerender_ticks = u32::MAX; // 二度と戻ってこない

    let (last, log) = drive(&mut nav, &mut reader);

    assert!(
        matches!(last, Action::Fail(Failure::PageDidNotRender { waited_ms }) if waited_ms >= 3_000),
        "{last:?}"
    );
    assert_eq!(count(&log, |a| matches!(a, Action::MeasurePage | Action::CapturePage { .. })), 0);
}

#[test]
fn fails_when_the_settings_menu_never_opens() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 10);
    reader.menu_operable = false;

    let (last, _) = drive(&mut nav, &mut reader);

    assert_eq!(last, Action::Fail(Failure::SettingsMenuUnavailable));
}

#[test]
fn fails_when_a_page_stops_advancing_before_the_end() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 100);
    reader.can_advance = false; // 巻き戻しも送りも効かない

    let (last, _) = drive(&mut nav, &mut reader);

    assert!(
        matches!(&last, Action::Fail(Failure::PageDidNotAdvance { at }) if at.current == 1),
        "{last:?}"
    );
}

/// 「位置」表示の書籍は、巻末に達しても位置が総数に届かない（ADR-0007 実測 5）。
/// これを故障と誤判定すると、全ページ撮り終えた書籍を捨ててしまう。
#[test]
fn finishes_a_location_style_book_without_confirming_the_end() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 10_167);
    reader.locations = true;
    reader.stop_at = Some(40); // 位置 40 が実際の巻末。総数 10167 には遠く届かない

    let (last, _) = drive(&mut nav, &mut reader);

    let Action::Done(summary) = last else { panic!("失敗にしてはいけない: {last:?}") };
    assert!(!summary.end_confirmed, "巻末を確定したと言ってはいけない");
    assert_eq!(summary.captured_pages, 40);
}

/// 同じ「途中で止まる」でも、ページ番号を持つ書籍なら故障と判定できる。
#[test]
fn a_page_numbered_book_that_stops_early_is_a_failure() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 100);
    reader.stop_at = Some(40);

    let (last, _) = drive(&mut nav, &mut reader);

    assert!(
        matches!(&last, Action::Fail(Failure::PageDidNotAdvance { at }) if at.current == 40),
        "{last:?}"
    );
}

#[test]
fn stops_at_the_page_cap_instead_of_running_away() {
    let limits = Limits { max_pages: 5, ..quick_limits() };
    let mut nav = Navigator::with_limits(spec(), limits);
    let mut reader = FakeReader::new(1, 10_000);

    let (last, _) = drive(&mut nav, &mut reader);

    assert_eq!(last, Action::Fail(Failure::TooManyPages { limit: 5 }));
}

/// 目標に届かない設定でも、校正を諦めて撮影に進む（1 冊を落とさない）。
#[test]
fn proceeds_with_the_best_effort_when_calibration_cannot_converge() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 3);
    reader.font_response = always_tiny;

    let (last, log) = drive(&mut nav, &mut reader);

    assert!(matches!(last, Action::Done(_)), "校正に失敗しても撮影はする: {last:?}");
    assert_eq!(nav.px_per_char(), Some(12), "実測できた最後の値を記録する");
    let steps = count(&log, |a| matches!(a, Action::SetFontSize { .. }));
    assert!(steps <= usize::from(DisplayTarget::default().max_calibration_attempts));
}

#[test]
fn keeps_returning_the_same_result_after_it_has_ended() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(1, 2);
    let (last, _) = drive(&mut nav, &mut reader);

    assert!(nav.is_ended());
    let again = nav.step(&reader.observe());
    assert_eq!(again, last, "終端後に呼んでも同じ結果を返し続ける");
    assert_eq!(nav.step(&reader.observe()), last);
}

/// 記録・再生ハーネス（ADR-0001 §6b）の土台。行動列は JSON で保存・復元できる。
#[test]
fn the_action_log_can_be_persisted_as_a_fixture() {
    let mut nav = Navigator::with_limits(spec(), quick_limits());
    let mut reader = FakeReader::new(3, 5);
    let (_, log) = drive(&mut nav, &mut reader);

    let json = serde_json::to_string(&log).expect("行動列は直列化できる");
    let back: Vec<Action> = serde_json::from_str(&json).expect("復元できる");
    assert_eq!(back, log);
}
