//! フォントサイズ校正。
//!
//! リーダーのフォントサイズはスライダーでしか変えられず、**現在値を読み取れない**
//! （ADR-0005 実測 2）。そのため設定は冪等にできず、
//! 「スライダーを動かす → 1 ページ実測する → 目標範囲か確認する」を
//! 繰り返して収束させる。
//!
//! スライダー位置に対する画素/文字は単調非減少である（ADR-0005 実測 3 の
//! 5% → 27、35% → 27、65% → 45、95% → 51）。したがって二分探索が使える。
//! 5% と 35% が同値であることから分かるとおりスライダーは離散段階を持つので、
//! 探索区間が潰れても収束しない場合があり、試行回数の上限が必要になる。

use ke_core::DisplayTarget;

/// 校正 1 回分の判断。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// 目標範囲に入った。値は採用した画素/文字。
    Satisfied(u32),
    /// まだ範囲外。次に試すスライダー位置。
    Retry(f32),
    /// 試行回数を使い切った。値は最後に実測した画素/文字。
    GiveUp(u32),
}

impl Step {
    /// 次に試すスライダー位置（`Retry` のときのみ）。
    #[must_use]
    pub fn retry_fraction(self) -> Option<f32> {
        match self {
            Self::Retry(f) => Some(f),
            _ => None,
        }
    }
}

/// スライダー位置の二分探索。
#[derive(Debug, Clone)]
pub struct Calibrator {
    target: DisplayTarget,
    lo: f32,
    hi: f32,
    current: f32,
    attempts: u8,
}

impl Calibrator {
    /// 目標を与えて校正を始める。最初は中央から試す。
    #[must_use]
    pub fn new(target: DisplayTarget) -> Self {
        Self { target, lo: 0.0, hi: 1.0, current: 0.5, attempts: 0 }
    }

    /// いま試すべきスライダー位置。
    #[must_use]
    pub fn fraction(&self) -> f32 {
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
        if self.target.needs_larger(px_per_char) {
            self.lo = self.current;
        } else {
            self.hi = self.current;
        }
        self.current = f32::midpoint(self.lo, self.hi);
        Step::Retry(self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0005 実測 3 のスライダー位置と画素/文字の対応を再現する。
    /// 段階的で、5% と 35% は同値になる。
    fn measured(fraction: f32) -> u32 {
        match fraction {
            f if f < 0.5 => 27,
            f if f < 0.8 => 45,
            _ => 51,
        }
    }

    fn run(target: DisplayTarget) -> (Step, u8) {
        let mut c = Calibrator::new(target);
        loop {
            let step = c.observe(measured(c.fraction()));
            match step {
                Step::Retry(f) => assert!((0.0..=1.0).contains(&f)),
                other => return (other, c.attempts()),
            }
        }
    }

    #[test]
    fn converges_on_the_balanced_target() {
        let (step, attempts) = run(DisplayTarget::balanced());
        assert_eq!(step, Step::Satisfied(45));
        assert!(attempts <= 3, "{attempts} 回もかかるべきではない");
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
    fn moves_the_slider_up_when_characters_are_too_small() {
        let mut c = Calibrator::new(DisplayTarget::balanced());
        let before = c.fraction();
        let next = c.observe(27).retry_fraction().expect("まだ範囲外なので再試行になる");
        assert!(next > before, "小さすぎるならスライダーは右へ動くべき");
    }

    #[test]
    fn moves_the_slider_down_when_characters_are_too_large() {
        let mut c = Calibrator::new(DisplayTarget::balanced());
        let before = c.fraction();
        let next = c.observe(80).retry_fraction().expect("まだ範囲外なので再試行になる");
        assert!(next < before, "大きすぎるならスライダーは左へ動くべき");
    }

    /// 目標が達成できない設定でも、試行回数の上限で必ず止まる。
    #[test]
    fn gives_up_instead_of_looping_forever() {
        let impossible = DisplayTarget {
            min_px_per_char: 900,
            max_px_per_char: 999,
            ..DisplayTarget::balanced()
        };
        let (step, attempts) = run(impossible);
        assert!(matches!(step, Step::GiveUp(_)));
        assert_eq!(attempts, impossible.max_calibration_attempts);
    }

    #[test]
    fn stops_immediately_when_the_first_measurement_already_fits() {
        let mut c = Calibrator::new(DisplayTarget::balanced());
        assert_eq!(c.observe(45), Step::Satisfied(45));
        assert_eq!(c.attempts(), 1);
    }
}
