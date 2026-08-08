//! Kindle Cloud Reader を操作するナビゲーション状態機械。
//!
//! この crate は **I/O を一切持たない**。時計もファイルもネットワークも触らず、
//! [`Observation`] を受け取って次の 1 手 [`Action`] を返すだけである。
//! 実際にブラウザを触るのは `ke-cdp` の責務。
//!
//! この分離により、巻き戻しの終端判定・フォント校正・ページ送りの
//! 全分岐を**実機ゼロでテストできる**（ADR-0001 §6a）。
//! 派生元の `kindle_shot` が最も複雑（循環的複雑度 29）かつ
//! テスト不能だった層に相当する。
//!
//! # 全体の流れ
//!
//! ```text
//! 本を開く → 読み込み待ち → 設定メニュー → テーマ設定 → フォント校正 ⇄ 実測
//!                                                            ↓ 収束
//!                                             先頭へ巻き戻し → 撮影 ⇄ ページ送り → 完了
//! ```

#![forbid(unsafe_code)]

mod calibrate;

pub use calibrate::{Calibrator, Step as CalibrationStep};

use ke_core::{Action, BookSpec, Failure, Observation, PageLabel, Summary, WaitReason};

/// 各種の打ち切り条件。実機の挙動に合わせて調整する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// 本が開くのを待つ上限（ミリ秒）。
    pub book_load_timeout_ms: u64,
    /// ページ送りの反映を待つ上限（ミリ秒）。
    pub page_turn_timeout_ms: u64,
    /// 設定メニューの開閉を試みる回数。
    pub menu_attempts: u8,
    /// 巻き戻しで「位置が変わらない」を何回連続したら先頭とみなすか。
    pub rewind_stall_threshold: u8,
    /// 巻き戻しでキーを押す回数の上限。
    pub max_rewind_presses: u32,
    /// 1 冊あたりの撮影枚数の上限（暴走の保険）。
    pub max_pages: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            book_load_timeout_ms: 45_000,
            page_turn_timeout_ms: 8_000,
            menu_attempts: 3,
            rewind_stall_threshold: 2,
            max_rewind_presses: 2_000,
            max_pages: 5_000,
        }
    }
}

/// 待ち時間（ミリ秒）。観測の間隔でもある。
const WAIT_LOAD_MS: u32 = 1_000;
const WAIT_MENU_MS: u32 = 600;
const WAIT_APPLY_MS: u32 = 1_500;
const WAIT_TURN_MS: u32 = 250;

#[derive(Debug, Clone, PartialEq)]
enum State {
    Start,
    LoadingBook { waited_ms: u64 },
    MenuOpening { attempts: u8 },
    MenuOpenWait { attempts: u8 },
    ThemeWait,
    FontWait,
    MenuClosing { attempts: u8 },
    MenuCloseWait { attempts: u8 },
    Measuring,
    Rewinding { presses: u32, stalls: u8, from: Box<Observation> },
    RewindWait { presses: u32, stalls: u8, from: Box<Observation> },
    Capturing,
    PageTurning { from: Box<Observation>, waited_ms: u64 },
    Ended,
}

/// 1 冊分のナビゲーションを進める状態機械。
#[derive(Debug, Clone)]
pub struct Navigator {
    spec: BookSpec,
    limits: Limits,
    state: State,
    calibrator: Calibrator,
    captured: u32,
    px_per_char: Option<u32>,
    /// 終端に達したときの行動。以降はこれを返し続ける。
    outcome: Option<Action>,
}

impl Navigator {
    /// 書籍指定から状態機械を作る。
    #[must_use]
    pub fn new(spec: BookSpec) -> Self {
        Self::with_limits(spec, Limits::default())
    }

    /// 打ち切り条件を指定して作る。
    #[must_use]
    pub fn with_limits(spec: BookSpec, limits: Limits) -> Self {
        let calibrator = Calibrator::new(spec.display);
        Self {
            spec,
            limits,
            state: State::Start,
            calibrator,
            captured: 0,
            px_per_char: None,
            outcome: None,
        }
    }

    /// これまでに撮影したページ数。
    #[must_use]
    pub fn captured_pages(&self) -> u32 {
        self.captured
    }

    /// 校正で採用した画素/文字。まだ校正していなければ `None`。
    #[must_use]
    pub fn px_per_char(&self) -> Option<u32> {
        self.px_per_char
    }

    /// 終端に達しているか。
    #[must_use]
    pub fn is_ended(&self) -> bool {
        self.outcome.is_some()
    }

    /// 終端に達したときの行動。まだなら `None`。
    #[must_use]
    pub fn outcome(&self) -> Option<&Action> {
        self.outcome.as_ref()
    }

    /// 観測を 1 つ受け取り、次の 1 手を返す。
    ///
    /// 終端に達したあとに呼ぶと、**同じ終端の行動を返し続ける**。
    /// 呼び出し側が誤って回し続けても結果が変質しない。
    pub fn step(&mut self, obs: &Observation) -> Action {
        if let Some(done) = &self.outcome {
            return done.clone();
        }
        let (action, next) = self.transition(obs);
        if action.is_terminal() {
            self.outcome = Some(action.clone());
        }
        self.state = next;
        action
    }

    /// 準備側（本を開く〜フォント校正）の遷移。
    fn transition(&mut self, obs: &Observation) -> (Action, State) {
        match self.state.clone() {
            State::Start => self.on_start(),
            State::LoadingBook { waited_ms } => self.on_loading(obs, waited_ms),
            State::MenuOpening { attempts } => self.on_menu_opening(obs, attempts),
            State::MenuOpenWait { attempts } => {
                (wait(WAIT_MENU_MS, WaitReason::SettingsMenu), State::MenuOpening { attempts })
            }
            // テーマを変えたら設定メニューの判定に戻る。次はフォントの番になる。
            State::ThemeWait => (
                wait(WAIT_APPLY_MS, WaitReason::SettingsApplied),
                State::MenuOpening { attempts: 1 },
            ),
            State::FontWait => (
                wait(WAIT_APPLY_MS, WaitReason::SettingsApplied),
                State::MenuClosing { attempts: 0 },
            ),
            other => self.transition_capture(obs, other),
        }
    }

    /// 撮影側（メニューを閉じる以降）の遷移。
    fn transition_capture(&mut self, obs: &Observation, state: State) -> (Action, State) {
        match state {
            State::MenuClosing { attempts } => self.on_menu_closing(obs, attempts),
            State::MenuCloseWait { attempts } => {
                (wait(WAIT_MENU_MS, WaitReason::SettingsMenu), State::MenuClosing { attempts })
            }
            State::Measuring => self.on_measuring(obs),
            State::Rewinding { presses, stalls, from } => {
                self.on_rewinding(obs, presses, stalls, &from)
            }
            State::RewindWait { presses, stalls, from } => (
                wait(WAIT_TURN_MS, WaitReason::PageTurn),
                State::Rewinding { presses, stalls, from },
            ),
            State::Capturing => self.on_capturing(obs),
            State::PageTurning { from, waited_ms } => self.on_page_turning(obs, &from, waited_ms),
            _ => (self.finish(false), State::Ended),
        }
    }

    fn on_start(&self) -> (Action, State) {
        (Action::OpenBook { url: self.spec.asin.reader_url() }, State::LoadingBook { waited_ms: 0 })
    }

    fn on_loading(&self, obs: &Observation, waited_ms: u64) -> (Action, State) {
        if obs.is_book_ready() {
            return (Action::OpenSettingsMenu, State::MenuOpenWait { attempts: 1 });
        }
        let waited = waited_ms.saturating_add(obs.elapsed_ms);
        if waited >= self.limits.book_load_timeout_ms {
            return (fail(Failure::BookDidNotLoad { waited_ms: waited }), State::Ended);
        }
        (wait(WAIT_LOAD_MS, WaitReason::BookLoading), State::LoadingBook { waited_ms: waited })
    }

    fn on_menu_opening(&self, obs: &Observation, attempts: u8) -> (Action, State) {
        if obs.settings_menu_open && obs.font_slider.is_some() {
            if obs.theme != Some(self.spec.display.theme) {
                return (Action::SetTheme(self.spec.display.theme), State::ThemeWait);
            }
            let fraction = self.calibrator.fraction();
            return (Action::ClickFontSlider { fraction }, State::FontWait);
        }
        if attempts >= self.limits.menu_attempts {
            return (fail(Failure::SettingsMenuUnavailable), State::Ended);
        }
        (Action::OpenSettingsMenu, State::MenuOpenWait { attempts: attempts.saturating_add(1) })
    }

    fn on_menu_closing(&self, obs: &Observation, attempts: u8) -> (Action, State) {
        if !obs.settings_menu_open {
            return (Action::MeasurePage, State::Measuring);
        }
        if attempts >= self.limits.menu_attempts {
            return (fail(Failure::SettingsMenuUnavailable), State::Ended);
        }
        (Action::CloseSettingsMenu, State::MenuCloseWait { attempts: attempts.saturating_add(1) })
    }

    fn on_measuring(&mut self, obs: &Observation) -> (Action, State) {
        let Some(m) = obs.metrics else {
            // 実測がまだ返っていない。もう一度観測する。
            return (wait(WAIT_APPLY_MS, WaitReason::SettingsApplied), State::Measuring);
        };
        match self.calibrator.observe(m.px_per_char) {
            CalibrationStep::Satisfied(px) | CalibrationStep::GiveUp(px) => {
                self.px_per_char = Some(px);
                self.start_rewind(obs)
            }
            CalibrationStep::Retry(_) => {
                (Action::OpenSettingsMenu, State::MenuOpenWait { attempts: 1 })
            }
        }
    }

    fn start_rewind(&self, obs: &Observation) -> (Action, State) {
        if obs.page.as_ref().is_some_and(PageLabel::is_first) {
            return (self.capture_here(obs), State::Capturing);
        }
        let from = Box::new(obs.clone());
        (Action::PressPrev, State::RewindWait { presses: 1, stalls: 0, from })
    }
}

/// 巻き戻しと撮影。
impl Navigator {
    fn on_rewinding(
        &self,
        obs: &Observation,
        presses: u32,
        stalls: u8,
        from: &Observation,
    ) -> (Action, State) {
        if obs.page.as_ref().is_some_and(PageLabel::is_first) {
            return (self.capture_here(obs), State::Capturing);
        }
        let stalls = if obs.advanced_from(from) { 0 } else { stalls.saturating_add(1) };
        if stalls >= self.limits.rewind_stall_threshold {
            // 位置が動かなくなった。先頭とみなして撮影に移る。
            return (self.capture_here(obs), State::Capturing);
        }
        if presses >= self.limits.max_rewind_presses {
            return (fail(Failure::RewindDidNotReachStart { presses }), State::Ended);
        }
        let next = State::RewindWait {
            presses: presses.saturating_add(1),
            stalls,
            from: Box::new(obs.clone()),
        };
        (Action::PressPrev, next)
    }

    /// 現在位置を撮る行動。位置表示が無い書籍では 1 起点の連番で代用する。
    fn capture_here(&self, obs: &Observation) -> Action {
        let label = obs
            .page
            .clone()
            .unwrap_or_else(|| PageLabel::new(self.captured.saturating_add(1), None));
        Action::CapturePage { label }
    }

    fn on_capturing(&mut self, obs: &Observation) -> (Action, State) {
        self.captured = self.captured.saturating_add(1);
        if self.captured >= self.limits.max_pages {
            return (fail(Failure::TooManyPages { limit: self.limits.max_pages }), State::Ended);
        }
        if obs.page.as_ref().is_some_and(PageLabel::is_last) {
            return (self.finish(true), State::Ended);
        }
        (Action::PressNext, State::PageTurning { from: Box::new(obs.clone()), waited_ms: 0 })
    }

    fn on_page_turning(
        &mut self,
        obs: &Observation,
        from: &Observation,
        waited_ms: u64,
    ) -> (Action, State) {
        if obs.advanced_from(from) && obs.has_usable_image() {
            return (self.capture_here(obs), State::Capturing);
        }
        let waited = waited_ms.saturating_add(obs.elapsed_ms);
        if waited < self.limits.page_turn_timeout_ms {
            let next = State::PageTurning { from: Box::new(from.clone()), waited_ms: waited };
            return (wait(WAIT_TURN_MS, WaitReason::PageTurn), next);
        }
        self.on_stall(from)
    }

    /// ページが進まなくなったときの扱い。
    ///
    /// 総ページ数が分かる書籍なら、巻末に達していないのに止まったのは故障である。
    /// 総ページ数が分からない書籍では巻末と故障を区別できないので、
    /// 打ち切って `end_confirmed: false` を立て、後段に判断を委ねる。
    fn on_stall(&self, from: &Observation) -> (Action, State) {
        match from.page.as_ref() {
            Some(p) if p.is_last() => (self.finish(true), State::Ended),
            Some(p) if p.total.is_some() => {
                (fail(Failure::PageDidNotAdvance { at: p.clone() }), State::Ended)
            }
            _ => (self.finish(false), State::Ended),
        }
    }

    fn finish(&self, end_confirmed: bool) -> Action {
        Action::Done(Summary {
            captured_pages: self.captured,
            px_per_char: self.px_per_char,
            end_confirmed,
        })
    }
}

fn wait(ms: u32, reason: WaitReason) -> Action {
    Action::Wait { ms, reason }
}

fn fail(f: Failure) -> Action {
    Action::Fail(f)
}
