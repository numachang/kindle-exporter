//! フェーズ実行の失敗。
//!
//! **リーダーの都合で撮り切れなかったこと（[`ke_core::Failure`]）は
//! ここに入れない。** それは異常ではなく結果であり、
//! [`crate::Outcome::Failed`] としてイベントログに残る。

use std::fmt;

/// この crate の結果型。
pub type Result<T> = std::result::Result<T, Error>;

/// フェーズを走らせられなかった理由。
#[derive(Debug)]
pub enum Error {
    /// ブラウザ操作に失敗した。
    Browser(ke_cdp::Error),
    /// 保管庫の読み書きに失敗した。
    Store(ke_store::Error),
    /// 実測に失敗した。
    Measure(String),
    /// 状態機械が終わらなかった（暴走の保険）。
    TooManySteps {
        /// 上限。
        limit: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Browser(e) => write!(f, "ブラウザ操作: {e}"),
            Self::Store(e) => write!(f, "保管庫: {e}"),
            Self::Measure(m) => write!(f, "実測: {m}"),
            Self::TooManySteps { limit } => {
                write!(f, "{limit} 手を超えても終わりませんでした")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Browser(e) => Some(e),
            Self::Store(e) => Some(e),
            Self::Measure(_) | Self::TooManySteps { .. } => None,
        }
    }
}

impl From<ke_cdp::Error> for Error {
    fn from(e: ke_cdp::Error) -> Self {
        Self::Browser(e)
    }
}

impl From<ke_store::Error> for Error {
    fn from(e: ke_store::Error) -> Self {
        Self::Store(e)
    }
}
