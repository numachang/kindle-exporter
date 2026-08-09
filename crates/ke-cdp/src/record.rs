//! セッションの記録と再生。
//!
//! **これが本設計の主眼である**（ADR-0001 §6b）。派生元の `kindle_shot` が
//! 構造的にできなかったのは、事故が起きたときにその状況を持ち帰ることだった。
//! 状態機械から I/O を追い出し、ブラウザを trait で切った目的はここにある。
//!
//! 実機で事故が起きたら、そのセッションの JSONL を
//! `crates/ke-cdp/fixtures/sessions/` に置くだけで回帰テストになる。
//!
//! # 何を記録し、何を記録しないか
//!
//! 記録するのは **[`Observation`] と [`Action`] だけ**である。
//! **ページ画像は記録しない。** 書籍の中身なので、公開リポジトリに置ける
//! フィクスチャにならなくなる。再生でも画像の中身は返らない（[`ReplayBrowser`]）。
//!
//! 実在の書籍を指さないよう、[`Session::redacted`] で ASIN を伏せてから保存する。
//!
//! # 既知の制約
//!
//! **記録は、それを取ったときと同じ打ち切り条件（`ke-nav` の `Limits`）でしか
//! 再生できない。** 条件が変われば判断も変わるので、ずれとして報告されてしまう。
//! 記録側に条件を持たせるのが本筋だが、`Limits` は `ke-nav` の型であり
//! この crate からは見えない。当面はフィクスチャを既定条件で取ることで揃える。

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use ke_core::{Action, Observation, PageImageInfo, Theme};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::{Browser, Direction, PageImage};

/// フィクスチャに書く伏せ字の書籍 URL。
const REDACTED_URL: &str = "https://read.amazon.co.jp/?asin=B0TESTBOOK";

/// 記録した 1 手。「こう見えたので、こう決めた」の対。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// 状態機械が見た観測。
    ///
    /// **実測値を差し込んだ後の姿**を記録すること。そうでないと再生時に
    /// 同じ判断を再現できない。
    pub observation: Observation,
    /// それを見て決めた行動。
    pub action: Action,
}

/// 1 セッション分の記録。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    steps: Vec<Step>,
}

/// 記録と再生がずれた箇所。
#[derive(Debug, Clone, PartialEq)]
pub struct Divergence {
    /// 何手目でずれたか（0 起点）。
    pub at: usize,
    /// 記録されていた行動。
    pub recorded: Action,
    /// いまの状態機械が決めた行動。
    pub produced: Action,
}

impl std::fmt::Display for Divergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} 手目で判断が変わりました: 記録は {:?} / いまは {:?}",
            self.at + 1,
            self.recorded,
            self.produced
        )
    }
}

impl Session {
    /// 空の記録を作る。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 1 手を記録する。
    pub fn push(&mut self, observation: Observation, action: Action) {
        self.steps.push(Step { observation, action });
    }

    /// 記録した全手。
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// 記録した手数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// 何も記録していないか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// 観測の並び。状態機械にそのまま食わせられる。
    pub fn observations(&self) -> impl Iterator<Item = &Observation> {
        self.steps.iter().map(|s| &s.observation)
    }

    /// 行動の並び。
    pub fn actions(&self) -> impl Iterator<Item = &Action> {
        self.steps.iter().map(|s| &s.action)
    }

    /// いまの状態機械が出した行動列と突き合わせ、**最初にずれた箇所**を返す。
    ///
    /// ずれていなければ `None`。手数が足りない場合もずれとして報告する。
    #[must_use]
    pub fn diverged_from(&self, produced: &[Action]) -> Option<Divergence> {
        for (at, step) in self.steps.iter().enumerate() {
            match produced.get(at) {
                Some(now) if *now == step.action => {}
                Some(now) => {
                    return Some(Divergence {
                        at,
                        recorded: step.action.clone(),
                        produced: now.clone(),
                    });
                }
                // 記録より手数が短い。途中で終わっている。
                None => {
                    return Some(Divergence {
                        at,
                        recorded: step.action.clone(),
                        produced: Action::Fail(ke_core::Failure::SettingsMenuUnavailable),
                    });
                }
            }
        }
        None
    }

    /// 実在の書籍を指さないよう、開いた URL を伏せた複製を返す。
    ///
    /// フィクスチャとしてリポジトリに置く前に必ず通すこと。
    #[must_use]
    pub fn redacted(&self) -> Self {
        let steps = self
            .steps
            .iter()
            .map(|s| Step {
                observation: s.observation.clone(),
                action: match &s.action {
                    Action::OpenBook { .. } => Action::OpenBook { url: REDACTED_URL.to_owned() },
                    other => other.clone(),
                },
            })
            .collect();
        Self { steps }
    }

    /// JSON Lines として書き出す（1 行 1 手）。
    ///
    /// 行単位なので、途中で落ちてもそこまでは読める。
    pub fn write_jsonl<W: Write>(&self, out: W) -> Result<()> {
        let mut out = BufWriter::new(out);
        for step in &self.steps {
            let line = serde_json::to_string(step)
                .map_err(|e| Error::Unexpected(format!("記録を直列化できません: {e}")))?;
            writeln!(out, "{line}")
                .map_err(|e| Error::Transport(format!("記録を書けません: {e}")))?;
        }
        out.flush().map_err(|e| Error::Transport(format!("記録を書けません: {e}")))
    }

    /// JSON Lines から読み込む。壊れた行は**その行番号つきで**報告する。
    pub fn read_jsonl<R: BufRead>(input: R) -> Result<Self> {
        let mut steps = Vec::new();
        for (at, line) in input.lines().enumerate() {
            let line = line.map_err(|e| Error::Transport(format!("記録を読めません: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }
            let step = serde_json::from_str(&line).map_err(|e| {
                Error::Unexpected(format!("記録の {} 行目を読めません: {e}", at + 1))
            })?;
            steps.push(step);
        }
        Ok(Self { steps })
    }

    /// ファイルに保存する。
    pub fn save(&self, path: &Path) -> Result<()> {
        let file = File::create(path)
            .map_err(|e| Error::Transport(format!("{} を作れません: {e}", path.display())))?;
        self.write_jsonl(file)
    }

    /// ファイルから読み込む。
    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .map_err(|e| Error::Transport(format!("{} を開けません: {e}", path.display())))?;
        Self::read_jsonl(BufReader::new(file))
    }
}

/// 記録した観測を順に返す [`Browser`]。
///
/// **ページ画像の中身は返らない。** 記録していないからである（本ファイル冒頭）。
/// 寸法と出所だけを持つ空の [`PageImage`] を返すので、
/// ナビゲーションの回帰テストには使えるが OCR には使えない。
#[derive(Debug, Clone)]
pub struct ReplayBrowser {
    steps: Vec<Step>,
    at: usize,
    theme: Option<Theme>,
}

impl ReplayBrowser {
    /// 記録から再生器を作る。
    #[must_use]
    pub fn new(session: &Session) -> Self {
        Self { steps: session.steps.to_vec(), at: 0, theme: None }
    }

    /// いま何手目か（0 起点）。
    #[must_use]
    pub fn position(&self) -> usize {
        self.at
    }

    /// 記録を最後まで再生し切ったか。
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.at >= self.steps.len()
    }

    fn current_image(&self) -> Option<&PageImageInfo> {
        self.steps.get(self.at.saturating_sub(1))?.observation.image.as_ref()
    }
}

impl Browser for ReplayBrowser {
    fn open(&mut self, _url: &str) -> Result<()> {
        Ok(())
    }

    fn observe(&mut self) -> Result<Observation> {
        let step = self.steps.get(self.at).ok_or_else(|| {
            Error::Unexpected(format!("記録は {} 手で尽きています", self.steps.len()))
        })?;
        self.at = self.at.saturating_add(1);
        Ok(step.observation.clone())
    }

    fn set_settings_menu(&mut self, _open: bool) -> Result<()> {
        Ok(())
    }

    fn set_theme(&mut self, theme: Theme) -> Result<()> {
        self.theme = Some(theme);
        Ok(())
    }

    fn set_font_size(&mut self, _index: u8) -> Result<()> {
        Ok(())
    }

    fn turn_page(&mut self, _direction: Direction) -> Result<()> {
        Ok(())
    }

    fn capture_page(&mut self) -> Result<PageImage> {
        let info = self
            .current_image()
            .ok_or_else(|| Error::NoPageImage("記録に画像の素性がありません".to_owned()))?;
        // 中身は記録していない。寸法と出所だけを返す。
        Ok(PageImage {
            png: Vec::new(),
            width: info.natural_width,
            height: info.natural_height,
            source: info.source.clone(),
        })
    }

    fn sleep(&mut self, _ms: u32) {
        // 再生では待たない。記録済みの観測が順に出てくるだけである。
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ke_core::{PageLabel, WaitReason};

    fn sample() -> Session {
        let mut s = Session::new();
        s.push(
            Observation::default(),
            Action::OpenBook { url: "https://read.amazon.co.jp/?asin=B0REALBOOK".into() },
        );
        s.push(
            Observation { elapsed_ms: 1_000, ..Observation::default() },
            Action::Wait { ms: 1_000, reason: WaitReason::BookLoading },
        );
        s.push(
            Observation {
                image: Some(PageImageInfo::ready(2199, 1692).with_source("blob:a")),
                page: Some(PageLabel::at_location(1, Some(10_167))),
                ..Observation::default()
            },
            Action::CapturePage { label: PageLabel::at_location(1, Some(10_167)) },
        );
        s
    }

    #[test]
    fn round_trips_through_json_lines() {
        let session = sample();
        let mut buffer = Vec::new();
        session.write_jsonl(&mut buffer).unwrap();

        assert_eq!(buffer.iter().filter(|b| **b == b'\n').count(), 3, "1 行 1 手");
        let back = Session::read_jsonl(buffer.as_slice()).unwrap();
        assert_eq!(back, session);
    }

    /// 公開リポジトリに置くので、実在の書籍を指す ASIN を残してはいけない。
    #[test]
    fn hides_the_book_it_was_recorded_from() {
        let redacted = sample().redacted();
        let json = serde_json::to_string(redacted.steps()).unwrap();
        assert!(!json.contains("B0REALBOOK"), "ASIN が残っている");
        assert!(json.contains("B0TESTBOOK"));
        // 伏せるのは URL だけ。判断の再現に要る観測は落とさない。
        assert_eq!(redacted.len(), sample().len());
        assert!(json.contains("10167"), "観測まで消してはいけない");
    }

    #[test]
    fn reports_where_the_decisions_started_to_differ() {
        let session = sample();
        let same: Vec<Action> = session.actions().cloned().collect();
        assert_eq!(session.diverged_from(&same), None);

        let mut changed = same.clone();
        changed[1] = Action::PressNext;
        let d = session.diverged_from(&changed).expect("ずれを見つける");
        assert_eq!(d.at, 1);
        assert_eq!(d.produced, Action::PressNext);
        assert!(d.to_string().contains("2 手目"), "{d}");
    }

    /// 記録より手数が短いのもずれである（途中で終わってしまった場合）。
    #[test]
    fn a_shorter_run_counts_as_a_divergence() {
        let session = sample();
        let cut: Vec<Action> = session.actions().take(1).cloned().collect();
        assert_eq!(session.diverged_from(&cut).map(|d| d.at), Some(1));
    }

    #[test]
    fn replays_the_recorded_observations_in_order() {
        let session = sample();
        let mut browser = ReplayBrowser::new(&session);

        for expected in session.observations() {
            assert_eq!(&browser.observe().unwrap(), expected);
        }
        assert!(browser.is_exhausted());
        assert!(browser.observe().is_err(), "尽きたら黙って繰り返さない");
    }

    /// 再生ではページ画像の中身は返らない。記録していないからである。
    #[test]
    fn replay_returns_page_dimensions_but_no_content() {
        let session = sample();
        let mut browser = ReplayBrowser::new(&session);
        for _ in 0..3 {
            browser.observe().unwrap();
        }
        let image = browser.capture_page().unwrap();
        assert_eq!((image.width, image.height), (2199, 1692));
        assert!(image.png.is_empty(), "書籍の中身は記録も再生もしない");
        assert_eq!(image.source.as_deref(), Some("blob:a"));
    }

    #[test]
    fn an_empty_line_in_the_log_is_skipped() {
        let text = "\n\n";
        assert!(Session::read_jsonl(text.as_bytes()).unwrap().is_empty());
    }

    /// 壊れた記録は、どの行が壊れているかまで言う。
    #[test]
    fn a_broken_line_is_reported_with_its_number() {
        let mut buffer = Vec::new();
        sample().write_jsonl(&mut buffer).unwrap();
        let mut text = String::from_utf8(buffer).unwrap();
        text.push_str("{ここが壊れている}\n");

        let err = Session::read_jsonl(text.as_bytes()).unwrap_err().to_string();
        assert!(err.contains("4 行目"), "{err}");
    }
}
