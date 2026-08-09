//! ナビゲーション状態機械が「決めること」。
//!
//! 状態機械は副作用を持たず、次にすべき 1 手を返すだけである。
//! 実際にブラウザを触るのは呼び出し側（`ke-cdp`）の責務。

use serde::{Deserialize, Serialize};

use crate::{PageLabel, Theme};

/// 待つ理由。ログと、テストで意図を確認するために持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    /// 本が開いて描画されるのを待っている。
    BookLoading,
    /// 設定メニューの開閉アニメーションを待っている。
    SettingsMenu,
    /// 表示設定の変更がページに反映されるのを待っている。
    SettingsApplied,
    /// ページ送り／戻しの反映を待っている。
    PageTurn,
}

/// 状態機械が次に行うべき 1 手。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// 指定 URL で本を開く。
    OpenBook {
        /// Cloud Reader の URL。
        url: String,
    },
    /// 指定時間だけ待ってから、再び観測する。
    Wait {
        /// 待つ時間（ミリ秒）。
        ms: u32,
        /// 待つ理由。
        reason: WaitReason,
    },
    /// 設定メニューを開く。
    OpenSettingsMenu,
    /// 設定メニューを閉じる。
    CloseSettingsMenu,
    /// 配色テーマを設定する。
    SetTheme(Theme),
    /// フォントサイズのスライダーを、左端からの割合で指定した位置にクリックする。
    ClickFontSlider {
        /// 0.0（最小）〜1.0（最大）。
        fraction: f32,
    },
    /// 現在のページを実測する（画素/文字を得るため）。
    MeasurePage,
    /// 現在のページ画像を保存する。
    CapturePage {
        /// 保存時に対応づけるページ位置。
        label: PageLabel,
    },
    /// 次のページへ送る。
    PressNext,
    /// 前のページへ戻す。
    PressPrev,
    /// 正常終了。
    Done(Summary),
    /// 中断。
    Fail(Failure),
}

/// 1 冊分の処理結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// 保存したページ数。
    pub captured_pages: u32,
    /// 最終的に採用した画素/文字。校正できなかった場合は `None`。
    pub px_per_char: Option<u32>,
    /// **巻末に到達したと確認できたか。**
    ///
    /// 総ページ数が分かる書籍では、最終ページに達したことを確認できる。
    /// 位置表示を持たない書籍では「巻末」と「ページ送りの故障」を区別できないため、
    /// ページが進まなくなった時点で打ち切り、ここを `false` にする。
    /// `validate` フェーズはこの値を見て、取りこぼしの疑いを報告する。
    pub end_confirmed: bool,
}

/// 中断の理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Failure {
    /// 制限時間内に本が開かなかった。
    BookDidNotLoad {
        /// 待った時間（ミリ秒）。
        waited_ms: u64,
    },
    /// 設定メニューを開けなかった。
    SettingsMenuUnavailable,
    /// ページ送りをしても位置が進まなくなった（末尾以外で）。
    PageDidNotAdvance {
        /// 止まった位置。
        at: PageLabel,
    },
    /// 先頭ページまで戻れなかった。
    RewindDidNotReachStart {
        /// 押した回数。
        presses: u32,
    },
    /// 撮影したページ数が想定を超えた（暴走の保険）。
    TooManyPages {
        /// 上限。
        limit: u32,
    },
}

impl Action {
    /// この行動で 1 冊分の処理が終わるか。
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done(_) | Self::Fail(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_done_and_fail_are_terminal() {
        let done = Summary { captured_pages: 1, px_per_char: Some(45), end_confirmed: true };
        assert!(Action::Done(done).is_terminal());
        assert!(Action::Fail(Failure::SettingsMenuUnavailable).is_terminal());
        assert!(!Action::PressNext.is_terminal());
        assert!(!Action::Wait { ms: 100, reason: WaitReason::PageTurn }.is_terminal());
    }

    /// 記録・再生ハーネス（ADR-0001 §6b）が成立することを固定する。
    #[test]
    fn actions_round_trip_through_json() {
        let actions = vec![
            Action::OpenBook { url: "https://read.amazon.co.jp/?asin=B0TESTBOOK".into() },
            Action::Wait { ms: 1200, reason: WaitReason::BookLoading },
            Action::ClickFontSlider { fraction: 0.65 },
            Action::MeasurePage,
            Action::CapturePage { label: PageLabel::new(1, Some(431)) },
            Action::Fail(Failure::PageDidNotAdvance { at: PageLabel::new(7, None) }),
        ];
        let json = serde_json::to_string(&actions).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Action>>(&json).unwrap(), actions);
    }
}
