//! フェーズと、それを実行できるホストの条件。
//!
//! ADR-0001 のフェーズ分割ワークフローと、ADR-0004 / ADR-0005 による更新
//! （`trim` は削除、`open` に表示設定の確定が入る）を反映している。

use std::fmt;

use serde::{Deserialize, Serialize};

/// ホストに求められる能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// どのホストでもよい。
    Any,
    /// ログイン済みの Chrome が必要（Mac 側で動かす想定）。
    Browser,
    /// CPU だけで完結する。
    Cpu,
    /// GPU が必要（Windows 側で動かす想定）。
    Gpu,
}

/// 1 冊を処理する工程。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// 蔵書から処理対象を決める。
    Plan,
    /// 本を開き、表示設定を確定し、先頭ページに合わせる。
    Open,
    /// ページ画像を原寸で取得する。
    Capture,
    /// 白紙・重複・欠落を機械検証する。
    Validate,
    /// レイアウト検出と文字列認識を行う。
    Ocr,
    /// ローカル LLM で校閲する。
    Proofread,
    /// 検索可能 PDF / Markdown を組み立てる。
    Assemble,
}

/// 実行順に並んだ全フェーズ。
pub const ALL_PHASES: [Phase; 7] = [
    Phase::Plan,
    Phase::Open,
    Phase::Capture,
    Phase::Validate,
    Phase::Ocr,
    Phase::Proofread,
    Phase::Assemble,
];

impl Phase {
    /// このフェーズを実行するのに必要なホストの能力。
    #[must_use]
    pub fn capability(self) -> Capability {
        match self {
            Self::Plan => Capability::Any,
            Self::Open | Self::Capture => Capability::Browser,
            Self::Validate | Self::Assemble => Capability::Cpu,
            Self::Ocr | Self::Proofread => Capability::Gpu,
        }
    }

    /// アーティファクト配置やイベントログで使う短い識別子。
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Open => "open",
            Self::Capture => "capture",
            Self::Validate => "validate",
            Self::Ocr => "ocr",
            Self::Proofread => "proofread",
            Self::Assemble => "assemble",
        }
    }

    /// 直前に完了している必要があるフェーズ。`Plan` なら `None`。
    #[must_use]
    pub fn predecessor(self) -> Option<Self> {
        let i = ALL_PHASES.iter().position(|p| *p == self)?;
        i.checked_sub(1).and_then(|j| ALL_PHASES.get(j).copied())
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_are_ordered_as_the_workflow_runs() {
        let mut sorted = ALL_PHASES;
        sorted.sort_unstable();
        assert_eq!(sorted, ALL_PHASES);
    }

    #[test]
    fn every_phase_has_a_unique_slug() {
        let mut slugs: Vec<_> = ALL_PHASES.iter().map(|p| p.slug()).collect();
        slugs.sort_unstable();
        let before = slugs.len();
        slugs.dedup();
        assert_eq!(slugs.len(), before);
    }

    /// Mac 側で撮り、Windows 側で OCR するという分担（ADR-0001）を固定する。
    #[test]
    fn browser_phases_are_capture_side_and_gpu_phases_are_ocr_side() {
        assert_eq!(Phase::Open.capability(), Capability::Browser);
        assert_eq!(Phase::Capture.capability(), Capability::Browser);
        assert_eq!(Phase::Ocr.capability(), Capability::Gpu);
        assert_eq!(Phase::Proofread.capability(), Capability::Gpu);
    }

    #[test]
    fn predecessors_form_a_single_chain() {
        assert_eq!(Phase::Plan.predecessor(), None);
        assert_eq!(Phase::Capture.predecessor(), Some(Phase::Open));
        assert_eq!(Phase::Assemble.predecessor(), Some(Phase::Proofread));
    }

    /// ADR-0004 で trim フェーズを削除したことを固定する。
    #[test]
    fn there_is_no_trim_phase() {
        assert!(!ALL_PHASES.iter().any(|p| p.slug() == "trim"));
        assert_eq!(Phase::Ocr.predecessor(), Some(Phase::Validate));
    }
}
