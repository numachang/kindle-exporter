//! 書籍の識別子と、1 冊を処理するための指定。

use std::fmt;

use serde::{Deserialize, Serialize};

/// Amazon の書籍識別子。
///
/// 生成時に形式を検証するため、`Asin` を持っていることが
/// 「URL に埋めても安全な文字列である」ことの証明になる。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Asin(String);

/// [`Asin`] の生成に失敗した理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsinError {
    /// 空文字列だった。
    Empty,
    /// 英数字以外の文字が含まれていた。
    NotAlphanumeric,
    /// 長さが想定外だった（ASIN は 10 文字）。
    BadLength(usize),
}

impl fmt::Display for AsinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "ASIN が空です"),
            Self::NotAlphanumeric => write!(f, "ASIN に英数字以外が含まれています"),
            Self::BadLength(n) => write!(f, "ASIN は 10 文字である必要があります（{n} 文字）"),
        }
    }
}

impl std::error::Error for AsinError {}

impl Asin {
    /// 文字列から `Asin` を作る。英数字 10 文字だけを受け付ける。
    pub fn new(s: impl Into<String>) -> Result<Self, AsinError> {
        let s = s.into();
        if s.is_empty() {
            return Err(AsinError::Empty);
        }
        if !s.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(AsinError::NotAlphanumeric);
        }
        if s.len() != 10 {
            return Err(AsinError::BadLength(s.len()));
        }
        Ok(Self(s.to_ascii_uppercase()))
    }

    /// 内部の文字列を借用する。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Cloud Reader でこの本を開く URL を返す。
    #[must_use]
    pub fn reader_url(&self) -> String {
        format!("https://read.amazon.co.jp/?asin={}", self.0)
    }
}

impl fmt::Display for Asin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Asin {
    type Error = AsinError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<Asin> for String {
    fn from(a: Asin) -> Self {
        a.0
    }
}

/// 1 冊を処理するための指定。`manifest.json` に保存される。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookSpec {
    /// 対象書籍。
    pub asin: Asin,
    /// 出力ファイル名やアーティファクト配置に使う表示名。
    pub title: String,
    /// キャプチャ時の表示設定。省略時は既定値。
    #[serde(default)]
    pub display: crate::DisplayTarget,
}

impl BookSpec {
    /// 既定の表示設定で 1 冊分の指定を作る。
    #[must_use]
    pub fn new(asin: Asin, title: impl Into<String>) -> Self {
        Self { asin, title: title.into(), display: crate::DisplayTarget::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_well_formed_asin_and_uppercases_it() {
        let a = Asin::new("b0bqqsqt86").unwrap();
        assert_eq!(a.as_str(), "B0BQQSQT86");
        assert_eq!(a.reader_url(), "https://read.amazon.co.jp/?asin=B0BQQSQT86");
    }

    #[test]
    fn rejects_malformed_asins() {
        assert_eq!(Asin::new(""), Err(AsinError::Empty));
        assert_eq!(Asin::new("B0BQ/SQT86"), Err(AsinError::NotAlphanumeric));
        assert_eq!(Asin::new("B0BQ"), Err(AsinError::BadLength(4)));
    }

    /// URL に注入されうる文字が型の時点で弾かれることを固定する。
    #[test]
    fn rejects_url_injection_attempts() {
        for bad in ["B0BQ&x=1AB", "B0BQ?x=1AB", "B0BQ#fragAB", "B0BQ AB1234"] {
            assert!(Asin::new(bad).is_err(), "{bad} を受け付けてはいけない");
        }
    }

    #[test]
    fn round_trips_through_json() {
        let spec = BookSpec::new(Asin::new("B0BQQSQT86").unwrap(), "テスト本");
        let json = serde_json::to_string(&spec).unwrap();
        let back: BookSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn rejects_malformed_asin_when_deserializing() {
        let err = serde_json::from_str::<BookSpec>(r#"{"asin":"oops","title":"x"}"#);
        assert!(err.is_err());
    }
}
