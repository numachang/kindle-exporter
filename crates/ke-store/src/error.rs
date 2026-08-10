//! 保管庫を触るときに起きうる失敗。
//!
//! 共有ストレージ上で複数のホストが同じ蔵書を触るため（ADR-0001 §3）、
//! **「誰が掴んでいるのか」まで返せること**を重視している。
//! 「busy」としか言わないロックは、実運用で手詰まりになる。

use std::fmt;
use std::path::PathBuf;

use ke_core::Phase;

use crate::lease::LeaseInfo;

/// この crate の結果型。
pub type Result<T> = std::result::Result<T, Error>;

/// 保管庫の失敗。
#[derive(Debug)]
pub enum Error {
    /// ファイル操作に失敗した。
    Io {
        /// 何をしようとしたか。
        what: &'static str,
        /// 対象のパス。
        path: PathBuf,
        /// OS が返した理由。
        message: String,
    },
    /// 記録の形が想定と違う。
    Corrupt {
        /// 対象のパス。
        path: PathBuf,
        /// どう違うか。
        message: String,
    },
    /// 他のホスト（か、落ちた自分自身）がそのフェーズを掴んでいる。
    Busy {
        /// 掴まれているフェーズ。
        phase: Phase,
        /// 掴んでいる側の素性。
        holder: Box<LeaseInfo>,
    },
}

impl Error {
    pub(crate) fn io(what: &'static str, path: impl Into<PathBuf>, e: &std::io::Error) -> Self {
        Self::Io { what, path: path.into(), message: e.to_string() }
    }

    pub(crate) fn corrupt(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Corrupt { path: path.into(), message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { what, path, message } => {
                write!(f, "{what}に失敗しました（{}）: {message}", path.display())
            }
            Self::Corrupt { path, message } => {
                write!(f, "{} の内容が壊れています: {message}", path.display())
            }
            Self::Busy { phase, holder } => {
                write!(f, "{phase} は {holder} が処理中です")
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 掴んでいる相手が分からないと、共有ストレージでは手が打てない。
    #[test]
    fn a_busy_phase_says_who_is_holding_it() {
        let holder = LeaseInfo { host: "mac-mini".into(), pid: 4321, at_unix_ms: 0 };
        let e = Error::Busy { phase: Phase::Capture, holder: Box::new(holder) };
        let s = e.to_string();
        assert!(s.contains("capture"), "{s}");
        assert!(s.contains("mac-mini"), "{s}");
        assert!(s.contains("4321"), "{s}");
    }
}
