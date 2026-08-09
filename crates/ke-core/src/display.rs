//! キャプチャ時のリーダー表示設定。
//!
//! ADR-0005 の実測に基づく。フォントサイズは OCR 精度の最大レバーであり
//! （画素/文字 27 → 51、ルビ列の幅 11px → 22px）、リーダーの設定 UI を
//! 操作しないと変えられない。
//!
//! フォントサイズは 0 から始まる離散段で表す（ADR-0007 実測 2 で 14 段と実測）。
//! **現在段は読み取れるので設定は冪等にできる**が、段と画素/文字の対応は
//! 書籍と viewport に依存するため、目標は画素/文字で指定し、
//! 「設定 → 実測 → 検証」で収束させる。

use serde::{Deserialize, Serialize};

/// リーダーの配色テーマ。
///
/// 明色にするとページ画像自体が白地黒字で配信されるため、
/// OCR 前の反転処理が不要になる（ADR-0005 実測 6）。
/// 実機の設定メニューは 4 択である（ADR-0007 実測 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// 白地黒字。OCR にはこれを使う。
    #[default]
    White,
    /// 黒地白字。取得後に反転が必要になるため通常は使わない。
    Dark,
    /// セピア。
    Sepia,
    /// 緑。
    Green,
}

/// フォントサイズの操作子について観測できたこと（ADR-0007 実測 2）。
///
/// リーダーは `ion-range` の `value` 属性に現在段を、`max` 属性に最大段を持つ。
/// **JS プロパティとしては読めない**ので、属性から読む必要がある。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontControl {
    /// 現在の段。
    pub index: u8,
    /// 最大の段（実機の実測値は 13。すなわち 0〜13 の 14 段）。
    pub max: u8,
}

impl FontControl {
    /// 現在段と最大段から作る。
    #[must_use]
    pub fn new(index: u8, max: u8) -> Self {
        Self { index, max }
    }

    /// 指定段を操作可能な範囲に丸める。
    #[must_use]
    pub fn clamp(&self, index: u8) -> u8 {
        index.min(self.max)
    }
}

/// フォントサイズ校正の目標。
///
/// 1 文字あたりの画素数で指定する。スライダーの位置ではなく実測値を
/// 目標にするのは、スライダーの段階数や刻みが書籍・環境で変わりうるため。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayTarget {
    /// 配色テーマ。
    pub theme: Theme,
    /// 画素/文字の下限（この値以上にする）。
    pub min_px_per_char: u32,
    /// 画素/文字の上限（この値以下にする）。大きすぎると撮影枚数が増える。
    pub max_px_per_char: u32,
    /// 校正の試行回数上限。超えたら最後の設定で続行する。
    pub max_calibration_attempts: u8,
}

impl Default for DisplayTarget {
    /// ADR-0005 決定 3 の既定（スライダー 65% 相当、画素/文字 45 前後）。
    fn default() -> Self {
        Self::balanced()
    }
}

impl DisplayTarget {
    /// 既定。精度と撮影枚数の釣り合いを取る（実測: 画素/文字 45、ルビ 20px、536 文字/ページ）。
    #[must_use]
    pub fn balanced() -> Self {
        Self {
            theme: Theme::White,
            min_px_per_char: 40,
            max_px_per_char: 50,
            max_calibration_attempts: 6,
        }
    }

    /// ルビ優先。ルビの多い書籍・難読語の多い書籍向け（実測: 画素/文字 51、ルビ 22px）。
    #[must_use]
    pub fn ruby_first() -> Self {
        Self { min_px_per_char: 48, max_px_per_char: 60, ..Self::balanced() }
    }

    /// 速度優先。ルビの無い実用書向け。撮影枚数が 4 分の 1 になる。
    #[must_use]
    pub fn fast() -> Self {
        Self { min_px_per_char: 26, max_px_per_char: 34, ..Self::balanced() }
    }

    /// 実測値が目標範囲に入っているか。
    #[must_use]
    pub fn is_satisfied_by(&self, px_per_char: u32) -> bool {
        (self.min_px_per_char..=self.max_px_per_char).contains(&px_per_char)
    }

    /// 実測値が目標より小さい（＝フォントを大きくする必要がある）か。
    #[must_use]
    pub fn needs_larger(&self, px_per_char: u32) -> bool {
        px_per_char < self.min_px_per_char
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_target_is_the_balanced_preset() {
        assert_eq!(DisplayTarget::default(), DisplayTarget::balanced());
        assert_eq!(DisplayTarget::default().theme, Theme::White);
    }

    #[test]
    fn classifies_measurements_against_the_target() {
        let t = DisplayTarget::balanced();
        assert!(t.needs_larger(27)); // ADR-0005: スライダー 5% の実測
        assert!(t.is_satisfied_by(45)); // 同 65%
        assert!(!t.is_satisfied_by(51)); // 同 95%（既定には大きすぎる）
        assert!(!t.needs_larger(51));
    }

    #[test]
    fn the_ruby_preset_accepts_the_largest_measured_setting() {
        assert!(DisplayTarget::ruby_first().is_satisfied_by(51));
    }

    #[test]
    fn the_fast_preset_accepts_the_smallest_measured_setting() {
        assert!(DisplayTarget::fast().is_satisfied_by(27));
    }

    #[test]
    fn presets_round_trip_through_json() {
        for t in [DisplayTarget::balanced(), DisplayTarget::ruby_first(), DisplayTarget::fast()] {
            let s = serde_json::to_string(&t).unwrap();
            assert_eq!(serde_json::from_str::<DisplayTarget>(&s).unwrap(), t);
        }
    }

    /// ADR-0007 実測 2 の実機の段数（0〜13 の 14 段）。
    #[test]
    fn clamps_font_steps_to_what_the_reader_offers() {
        let f = FontControl::new(5, 13);
        assert_eq!(f.clamp(8), 8);
        assert_eq!(f.clamp(13), 13);
        assert_eq!(f.clamp(99), 13, "無い段は最大段に丸める");
    }

    #[test]
    fn themes_round_trip_through_json() {
        for t in [Theme::White, Theme::Dark, Theme::Sepia, Theme::Green] {
            let s = serde_json::to_string(&t).unwrap();
            assert_eq!(serde_json::from_str::<Theme>(&s).unwrap(), t);
        }
    }
}
