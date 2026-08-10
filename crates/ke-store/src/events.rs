//! 追記専用のイベントログ。**1 冊の状態はこれが唯一の真実である**（ADR-0001 §3）。
//!
//! 1 行 1 イベントの JSON Lines にしてあるのは、
//!
//! - 追記だけなので、複数ホストが別々の書籍を触っても衝突しない
//! - 途中で電源が落ちても、そこまでの行は読める
//! - マージできる（同じ書籍を 2 台で触った場合も行を並べるだけ）
//!
//! ためである。ローカルの索引（SQLite 等）はここから再構築できるものに限る。

use std::collections::BTreeSet;
use std::io::BufRead;

use ke_core::{PageLabel, Phase, Summary};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::{host_name, now_unix_ms};

/// 起きたこと。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// 書籍を登録した。
    BookRegistered,
    /// フェーズを始めた。
    PhaseStarted {
        /// 始めたフェーズ。
        phase: Phase,
    },
    /// ページ画像を保存した。
    PageCaptured {
        /// 1 起点の連番。ファイル名と対応する。
        index: u32,
        /// リーダーが表示していた位置。持たない書籍では `None`。
        label: Option<PageLabel>,
        /// PNG のバイト数。
        bytes: u64,
    },
    /// フェーズが正常に終わった。
    PhaseFinished {
        /// 終わったフェーズ。
        phase: Phase,
        /// capture フェーズの結果。他のフェーズでは `None`。
        summary: Option<Summary>,
    },
    /// フェーズが失敗した。
    PhaseFailed {
        /// 失敗したフェーズ。
        phase: Phase,
        /// 失敗の理由（人が読むためのもの）。
        reason: String,
    },
}

/// イベント 1 件と、それが起きた状況。
///
/// **どのホストで起きたか**を残す。Mac で撮って Windows で OCR する構成なので
/// （ADR-0001 §2）、これが無いと後から追えない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// 発生時刻（UNIX ミリ秒）。
    pub at_unix_ms: u64,
    /// 発生したホスト。
    pub host: String,
    /// 起きたこと。
    #[serde(flatten)]
    pub event: Event,
}

impl Record {
    /// いま・ここで起きたこととして包む。
    #[must_use]
    pub fn here(event: Event) -> Self {
        Self { at_unix_ms: now_unix_ms(), host: host_name(), event }
    }
}

/// JSON Lines を読み、壊れた行は**行番号つきで**報告する。
pub(crate) fn read_records<R: BufRead>(input: R, path: &std::path::Path) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for (at, line) in input.lines().enumerate() {
        let line = line.map_err(|e| Error::io("イベントログの読み取り", path, &e))?;
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str(&line)
            .map_err(|e| Error::corrupt(path, format!("{} 行目: {e}", at + 1)))?;
        records.push(record);
    }
    Ok(records)
}

/// イベントログから読み取れる、1 冊のいまの状態。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    /// 保存済みのページ数。
    ///
    /// **同じ連番を数え直さない。** 途中から撮り直したときに二重に数えると、
    /// 実際より進んでいるように見えてしまう。
    pub captured_pages: u32,
    /// 最後に保存したページの連番。1 枚も無ければ `None`。
    pub last_page_index: Option<u32>,
    /// 完了したフェーズ。
    pub finished: Vec<Phase>,
    /// capture フェーズの結果。まだ終わっていなければ `None`。
    pub capture_summary: Option<Summary>,
}

impl Progress {
    /// イベント列から組み立てる。**これが再開の判断材料になる。**
    #[must_use]
    pub fn from_records(records: &[Record]) -> Self {
        let mut progress = Self::default();
        let mut pages = BTreeSet::new();
        for record in records {
            progress.apply(&record.event, &mut pages);
        }
        progress.captured_pages = u32::try_from(pages.len()).unwrap_or(u32::MAX);
        progress
    }

    fn apply(&mut self, event: &Event, pages: &mut BTreeSet<u32>) {
        match event {
            Event::PageCaptured { index, .. } => {
                pages.insert(*index);
                self.last_page_index = Some(*index);
            }
            Event::PhaseFinished { phase, summary } => {
                if !self.finished.contains(phase) {
                    self.finished.push(*phase);
                }
                if *phase == Phase::Capture {
                    self.capture_summary = *summary;
                }
            }
            Event::BookRegistered | Event::PhaseStarted { .. } | Event::PhaseFailed { .. } => {}
        }
    }

    /// そのフェーズは完了しているか。
    #[must_use]
    pub fn has_finished(&self, phase: Phase) -> bool {
        self.finished.contains(&phase)
    }

    /// 次に保存すべきページの連番。
    #[must_use]
    pub fn next_page_index(&self) -> u32 {
        self.last_page_index.map_or(1, |i| i.saturating_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(event: Event) -> Record {
        Record { at_unix_ms: 0, host: "test".into(), event }
    }

    #[test]
    fn round_trips_through_json_lines() {
        let records = vec![
            rec(Event::BookRegistered),
            rec(Event::PhaseStarted { phase: Phase::Capture }),
            rec(Event::PageCaptured {
                index: 1,
                label: Some(PageLabel::at_location(48, Some(10_167))),
                bytes: 601_234,
            }),
        ];
        let text: String = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect::<Vec<_>>()
            .concat();

        let back = read_records(text.as_bytes(), std::path::Path::new("x")).unwrap();
        assert_eq!(back, records);
    }

    /// 撮り直したページを二重に数えない。数えると実際より進んで見える。
    #[test]
    fn recapturing_a_page_does_not_count_it_twice() {
        let records = vec![
            rec(Event::PageCaptured { index: 1, label: None, bytes: 10 }),
            rec(Event::PageCaptured { index: 2, label: None, bytes: 10 }),
            // 途中で落ちて、最初から撮り直した
            rec(Event::PageCaptured { index: 1, label: None, bytes: 10 }),
            rec(Event::PageCaptured { index: 2, label: None, bytes: 10 }),
            rec(Event::PageCaptured { index: 3, label: None, bytes: 10 }),
        ];
        assert_eq!(Progress::from_records(&records).captured_pages, 3);
    }

    /// 再開の判断材料はイベントだけから作れること。
    #[test]
    fn progress_is_rebuilt_from_the_events_alone() {
        let records = vec![
            rec(Event::PhaseStarted { phase: Phase::Capture }),
            rec(Event::PageCaptured { index: 1, label: None, bytes: 10 }),
            rec(Event::PageCaptured { index: 2, label: None, bytes: 10 }),
        ];
        let p = Progress::from_records(&records);
        assert_eq!(p.captured_pages, 2);
        assert_eq!(p.next_page_index(), 3, "続きは 3 枚目から");
        assert!(!p.has_finished(Phase::Capture));
    }

    #[test]
    fn a_finished_capture_carries_its_summary() {
        let summary = Summary { captured_pages: 2, px_per_char: Some(45), end_confirmed: true };
        let records = vec![
            rec(Event::PageCaptured { index: 1, label: None, bytes: 10 }),
            rec(Event::PhaseFinished { phase: Phase::Capture, summary: Some(summary) }),
        ];
        let p = Progress::from_records(&records);
        assert!(p.has_finished(Phase::Capture));
        assert_eq!(p.capture_summary, Some(summary));
    }

    /// 1 枚も撮っていなければ 1 枚目から始める。
    #[test]
    fn a_fresh_book_starts_at_the_first_page() {
        assert_eq!(Progress::default().next_page_index(), 1);
    }

    #[test]
    fn a_broken_line_is_reported_with_its_number() {
        let text =
            "{\"at_unix_ms\":0,\"host\":\"t\",\"event\":\"book_registered\"}\nこわれている\n";
        let err = read_records(text.as_bytes(), std::path::Path::new("events.jsonl"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 行目"), "{err}");
    }
}
