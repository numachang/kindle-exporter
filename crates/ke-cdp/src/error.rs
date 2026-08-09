//! ブラウザ駆動で起きうる失敗。
//!
//! 実機で困るのは「何が起きたか分からない」ことなので、
//! **どの要素を探して何が見つかったか**まで持たせる。
//! リーダーの UI が変わったとき、エラーメッセージだけで原因に辿り着けるようにする。

use std::fmt;

/// この crate の結果型。
pub type Result<T> = std::result::Result<T, Error>;

/// ブラウザ駆動の失敗。
#[derive(Debug)]
pub enum Error {
    /// CDP のエンドポイントに繋がらない。
    Connect(String),
    /// WebSocket が切れた・壊れた。
    Transport(String),
    /// CDP がメソッド呼び出しをエラーで返した。
    Protocol {
        /// 呼び出したメソッド名。
        method: String,
        /// CDP が返した説明。
        message: String,
    },
    /// ページ内で評価した JS が例外を投げた。
    Script(String),
    /// 応答の形が想定と違う。
    Unexpected(String),
    /// 期待した操作子が見つからない。
    ///
    /// `seen` には**実際に見えたもの**を入れる。リーダーの表示言語が違う場合、
    /// ここを見れば対応表に足すべき文字列が分かる。
    ElementNotFound {
        /// 探していたもの。
        what: &'static str,
        /// 実際に見つかったものの一覧。
        seen: Vec<String>,
    },
    /// ページ画像を取り出せない。
    NoPageImage(String),
    /// 設定を変えようとしたが、いつまでも目標値にならない。
    SettingRejected {
        /// 何の設定か。
        what: &'static str,
        /// 目標値。
        wanted: String,
        /// 最後に観測した値。
        got: String,
    },
}

impl Error {
    /// 操作子が見つからないエラーを、見えたものの一覧つきで作る。
    pub(crate) fn not_found(what: &'static str, seen: impl IntoIterator<Item = String>) -> Self {
        Self::ElementNotFound { what, seen: seen.into_iter().collect() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(m) => write!(f, "CDP に接続できません: {m}"),
            Self::Transport(m) => write!(f, "CDP との通信が壊れました: {m}"),
            Self::Protocol { method, message } => write!(f, "CDP {method} が失敗: {message}"),
            Self::Script(m) => write!(f, "ページ内の JS が例外を投げました: {m}"),
            Self::Unexpected(m) => write!(f, "想定外の応答: {m}"),
            Self::ElementNotFound { what, seen } => {
                write!(f, "{what} が見つかりません（見えたもの: {}）", seen.join(", "))
            }
            Self::NoPageImage(m) => write!(f, "ページ画像を取り出せません: {m}"),
            Self::SettingRejected { what, wanted, got } => {
                write!(f, "{what} を {wanted} にできません（最後に観測したのは {got}）")
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表示言語が違う環境で詰まったとき、メッセージだけで原因に辿り着けること。
    #[test]
    fn a_missing_control_reports_what_was_actually_there() {
        let e = Error::not_found(
            "ページ送りボタン",
            ["Next Page".to_string(), "Previous Page".to_string()],
        );
        let s = e.to_string();
        assert!(s.contains("ページ送りボタン"), "{s}");
        assert!(s.contains("Next Page"), "{s}");
    }
}
