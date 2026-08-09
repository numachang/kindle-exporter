//! フォントサイズ校正。
//!
//! リーダーのフォントサイズは 0 から始まる離散段で、現在段は
//! `ion-range` の `value` **属性**から読める（ADR-0007 実測 2。実機は 0〜13 の 14 段）。
//! したがって設定そのものは冪等にできる。
//!
//! それでも校正が要るのは、**段と画素/文字の対応が書籍と viewport に依存する**
//! ためである。目標は段番号ではなく画素/文字で持ち、
//! 「段を設定する → 1 ページ実測する → 目標範囲か確認する」で収束させる。
//!
//! 段に対する画素/文字は単調非減少である（ADR-0005 実測 3 の
//! 5% → 27、35% → 27、65% → 45、95% → 51）。したがって二分探索が使え、
//! 14 段なら 4 回以内に必ず打ち切れる。
//! 5% と 35% が同値であることから分かるとおり平坦な区間があるので、
//! 探索区間が尽きても目標に入らないことがある。その場合は諦めて先へ進む。

use ke_core::DisplayTarget;

/// 校正 1 回分の判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// 目標範囲に入った。値は採用した画素/文字。
    Satisfied(u32),
    /// まだ範囲外。次に試すフォント段。
    Retry(u8),
    /// 探索区間か試行回数を使い切った。値は最後に実測した画素/文字。
    GiveUp(u32),
}

impl Step {
    /// 次に試すフォント段（`Retry` のときのみ）。
    #[must_use]
    pub fn retry_index(self) -> Option<u8> {
        match self {
            Self::Retry(i) => Some(i),
            _ => None,
        }
    }
}

/// フォント段の二分探索。
#[derive(Debug, Clone)]
pub struct Calibrator {
    target: DisplayTarget,
    lo: u8,
    hi: u8,
    current: u8,
    attempts: u8,
}

impl Calibrator {
    /// 目標と、リーダーが持つ最大段を与えて校正を始める。最初は中央から試す。
    #[must_use]
    pub fn new(target: DisplayTarget, max_index: u8) -> Self {
        Self { target, lo: 0, hi: max_index, current: max_index / 2, attempts: 0 }
    }

    /// いま試すべきフォント段。
    #[must_use]
    pub fn index(&self) -> u8 {
        self.current
    }

    /// これまでの試行回数。
    #[must_use]
    pub fn attempts(&self) -> u8 {
        self.attempts
    }

    /// 実測値を与え、次にどうするかを決める。
    pub fn observe(&mut self, px_per_char: u32) -> Step {
        self.attempts = self.attempts.saturating_add(1);
        if self.target.is_satisfied_by(px_per_char) {
            return Step::Satisfied(px_per_char);
        }
        if self.attempts >= self.target.max_calibration_attempts {
            return Step::GiveUp(px_per_char);
        }
        if self.narrow(self.target.needs_larger(px_per_char)) {
            Step::Retry(self.current)
        } else {
            Step::GiveUp(px_per_char)
        }
    }

    /// 探索区間を半分にする。試せる段が残っていなければ `false`。
    fn narrow(&mut self, need_larger: bool) -> bool {
        if need_larger {
            if self.current >= self.hi {
                return false;
            }
            self.lo = self.current.saturating_add(1);
        } else {
            if self.current <= self.lo {
                return false;
            }
            self.hi = self.current.saturating_sub(1);
        }
        self.current = self.lo + (self.hi - self.lo) / 2;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実機の段数（ADR-0007 実測 2）。
    const MAX_INDEX: u8 = 13;

    /// ADR-0005 実測 3 の割合を段番号に読み替えた応答。
    /// 既定段 5 で画素/文字 27（ADR-0007 実測 2 と一致）、
    /// 65% ≒ 段 8 で 45、95% ≒ 段 12 で 51。平坦な区間を持つ。
    fn measured(index: u8) -> u32 {
        match index {
            0..=6 => 27,
            7..=10 => 45,
            _ => 51,
        }
    }

    fn run(target: DisplayTarget) -> (Step, u8) {
        let mut c = Calibrator::new(target, MAX_INDEX);
        loop {
            let step = c.observe(measured(c.index()));
            match step {
                Step::Retry(i) => assert!(i <= MAX_INDEX, "段 {i} は存在しない"),
                other => return (other, c.attempts()),
            }
        }
    }

    #[test]
    fn converges_on_the_balanced_target() {
        let (step, attempts) = run(DisplayTarget::balanced());
        assert_eq!(step, Step::Satisfied(45));
        assert!(attempts <= 4, "14 段なら 4 回以内に収束する（{attempts} 回）");
    }

    #[test]
    fn converges_on_the_ruby_target() {
        assert_eq!(run(DisplayTarget::ruby_first()).0, Step::Satisfied(51));
    }

    #[test]
    fn converges_on_the_fast_target() {
        assert_eq!(run(DisplayTarget::fast()).0, Step::Satisfied(27));
    }

    #[test]
    fn moves_up_when_characters_are_too_small() {
        let mut c = Calibrator::new(DisplayTarget::balanced(), MAX_INDEX);
        let before = c.index();
        let next = c.observe(27).retry_index().expect("まだ範囲外なので再試行になる");
        assert!(next > before, "小さすぎるなら段を上げる");
    }

    #[test]
    fn moves_down_when_characters_are_too_large() {
        let mut c = Calibrator::new(DisplayTarget::balanced(), MAX_INDEX);
        let before = c.index();
        let next = c.observe(80).retry_index().expect("まだ範囲外なので再試行になる");
        assert!(next < before, "大きすぎるなら段を下げる");
    }

    /// 目標が達成できない設定でも、探索区間が尽きた時点で必ず止まる。
    #[test]
    fn gives_up_instead_of_looping_forever() {
        let impossible = DisplayTarget {
            min_px_per_char: 900,
            max_px_per_char: 999,
            ..DisplayTarget::balanced()
        };
        let (step, attempts) = run(impossible);
        assert!(matches!(step, Step::GiveUp(_)), "{step:?}");
        assert!(attempts <= impossible.max_calibration_attempts);
    }

    /// 段が 1 つしかないリーダーでも、無限に試し続けない。
    #[test]
    fn gives_up_when_there_is_only_one_step() {
        let mut c = Calibrator::new(DisplayTarget::balanced(), 0);
        assert_eq!(c.index(), 0);
        assert_eq!(c.observe(27), Step::GiveUp(27));
    }

    #[test]
    fn stops_immediately_when_the_first_measurement_already_fits() {
        let mut c = Calibrator::new(DisplayTarget::balanced(), MAX_INDEX);
        assert_eq!(c.observe(45), Step::Satisfied(45));
        assert_eq!(c.attempts(), 1);
    }
}
