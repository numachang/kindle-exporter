//! capture フェーズの実行。
//!
//! # 再開の考え方
//!
//! **途中から再開せず、先頭から撮り直す。** 一見もったいないが、こちらが正しい。
//!
//! - ページの連番は「先頭から数えて何枚目か」なので、同じ表示設定で撮り直せば
//!   同じ連番に同じページが入る。つまり**上書きは冪等である**
//! - 「N ページ目まで進める」には結局 N 回送る必要があり、撮り直しとの差は
//!   画像を取り出す 17ms 分しかない（ADR-0007 実測 6）
//! - リーダーの再開位置は当てにできない。読みかけの位置から始まるとは限らない
//!
//! 200 ページの撮り直しは実測 227ms/頁 で 45 秒。落ちたときだけ払う費用としては安い。

use ke_cdp::{Browser, Effect, Session, apply};
use ke_core::{Action, Failure, PageMetrics, Phase, Summary};
use ke_nav::{Limits, Navigator};
use ke_store::{Book, Event};

use crate::error::{Error, Result};
use crate::measure::Measurer;

/// capture フェーズの走らせ方。
#[derive(Debug, Clone)]
pub struct CaptureOptions {
    /// 状態機械の打ち切り条件。
    ///
    /// **記録は同じ条件でしか再生できない。** 変えると、既に取った記録を
    /// 回帰テストに使えなくなる。
    pub limits: Limits,
    /// 観測と行動を記録するか（ADR-0001 §6b）。
    pub record: bool,
    /// 既に完了していても撮り直すか。
    pub force: bool,
    /// 暴走の保険。これを超えたら失敗させる。
    pub max_steps: usize,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self { limits: Limits::default(), record: true, force: false, max_steps: 200_000 }
    }
}

/// capture フェーズの結果。
///
/// **`Failed` は異常ではない。** 位置表示を持たない書籍で送りが止まった、
/// といった「そういうこともある」結果であり、イベントログに残して次へ進む。
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// 撮り切った。
    Finished(Summary),
    /// 途中で打ち切った。**撮れた分は残っている。**
    Failed {
        /// 打ち切った理由。
        failure: Failure,
        /// そこまでに保存できた枚数。
        captured_pages: u32,
    },
    /// 既に完了していたので何もしなかった。
    AlreadyDone(Summary),
}

impl Outcome {
    /// 撮影したページ数。
    #[must_use]
    pub fn captured_pages(&self) -> u32 {
        match self {
            Self::Finished(s) | Self::AlreadyDone(s) => s.captured_pages,
            Self::Failed { captured_pages, .. } => *captured_pages,
        }
    }
}

/// 1 冊を撮る。
///
/// フェーズのロックを取り、イベントを残し、ページ画像を保管庫に置く。
/// **ロックは戻り値と一緒に落ちる**ので、呼び出し側は解放を気にしなくてよい。
pub fn capture<B: Browser + ?Sized, M: Measurer + ?Sized>(
    book: &Book,
    browser: &mut B,
    measurer: &mut M,
    options: &CaptureOptions,
) -> Result<Outcome> {
    let progress = book.progress()?;
    if !options.force {
        if let Some(done) =
            progress.capture_summary.filter(|_| progress.has_finished(Phase::Capture))
        {
            return Ok(Outcome::AlreadyDone(done));
        }
    }

    let _lease = book.lease(Phase::Capture)?;
    let spec = book.manifest()?;
    book.append(Event::PhaseStarted { phase: Phase::Capture })?;

    let mut run = Run {
        nav: Navigator::with_limits(spec, options.limits),
        record: Session::new(),
        pending: None,
        next_index: 1,
    };
    let last = run.drive(book, browser, measurer, options.max_steps)?;

    if options.record {
        save_record(book, &run.record)?;
    }
    finish(book, last, run.next_index.saturating_sub(1))
}

/// 走らせている間の状態。
struct Run {
    nav: Navigator,
    record: Session,
    /// 実測の結果。次の観測に差し込む。
    pending: Option<PageMetrics>,
    /// 次に保存するページの連番。
    next_index: u32,
}

impl Run {
    /// 終端に達するまで回し、最後の行動を返す。
    fn drive<B: Browser + ?Sized, M: Measurer + ?Sized>(
        &mut self,
        book: &Book,
        browser: &mut B,
        measurer: &mut M,
        max_steps: usize,
    ) -> Result<Action> {
        for _ in 0..max_steps {
            let mut obs = browser.observe()?;
            obs.metrics = self.pending.take();

            let action = self.nav.step(&obs);
            // 実測を差し込んだ後の観測を記録する。でないと再生できない。
            self.record.push(obs, action.clone());

            if let Some(done) = self.perform(book, browser, measurer, &action)? {
                return Ok(done);
            }
        }
        Err(Error::TooManySteps { limit: max_steps })
    }

    /// 1 手を実行する。終端なら `Some(その行動)`。
    fn perform<B: Browser + ?Sized, M: Measurer + ?Sized>(
        &mut self,
        book: &Book,
        browser: &mut B,
        measurer: &mut M,
        action: &Action,
    ) -> Result<Option<Action>> {
        match apply(browser, action)? {
            Effect::Terminal => return Ok(Some(action.clone())),
            Effect::ToMeasure(image) => self.pending = Some(measurer.measure(&image.png)?),
            Effect::Captured { label, image } => {
                book.save_page(self.next_index, &image.png, Some(label))?;
                self.next_index = self.next_index.saturating_add(1);
            }
            Effect::Nothing => {}
        }
        Ok(None)
    }
}

/// 記録を書籍の脇に残す。事故が起きたときに持ち帰るためのもの。
fn save_record(book: &Book, record: &Session) -> Result<()> {
    let dir = book.dir().join("sessions");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Measure(format!("記録置き場を作れません: {e}")))?;
    let name = format!("capture-{}.jsonl", record.len());
    // 実在の書籍を指さないよう ASIN を伏せる（そのままフィクスチャにできる）。
    Ok(record.redacted().save(&dir.join(name))?)
}

/// 終端の行動をイベントログに落とす。
fn finish(book: &Book, last: Action, captured_pages: u32) -> Result<Outcome> {
    match last {
        Action::Done(summary) => {
            book.append(Event::PhaseFinished { phase: Phase::Capture, summary: Some(summary) })?;
            Ok(Outcome::Finished(summary))
        }
        Action::Fail(failure) => {
            book.append(Event::PhaseFailed {
                phase: Phase::Capture,
                reason: format!("{failure:?}"),
            })?;
            Ok(Outcome::Failed { failure, captured_pages })
        }
        // 状態機械は終端としてこの 2 つしか返さない（Action::is_terminal）。
        other => Err(Error::Measure(format!("終端でない行動で終わりました: {other:?}"))),
    }
}
