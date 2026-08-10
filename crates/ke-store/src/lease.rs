//! フェーズの排他ロック。
//!
//! ADR-0001 §3 のとおり **SQLite は使わない。** ネットワーク共有
//! （SMB / Syncthing）上では SQLite のロックが正しく働かないためである。
//! 代わりに `O_EXCL` で作るファイル 1 個をロックとする。
//! これはネットワーク共有でも「作れたのは 1 人だけ」が成り立つ。
//!
//! **奪い取りは自動でやらない。** ホストが落ちるとロックのファイルは残るが、
//! それを勝手に消すと、実は生きていた相手と二重に走る。
//! 誰が掴んでいるかを返して、人間に判断させる（[`LeaseInfo`]）。

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use ke_core::Phase;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::{host_name, now_unix_ms};

/// ロックを掴んでいる側の素性。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseInfo {
    /// ホスト名。
    pub host: String,
    /// プロセス ID。
    pub pid: u32,
    /// 掴んだ時刻（UNIX ミリ秒）。
    pub at_unix_ms: u64,
}

impl LeaseInfo {
    fn here() -> Self {
        Self { host: host_name(), pid: std::process::id(), at_unix_ms: now_unix_ms() }
    }
}

impl fmt::Display for LeaseInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}（pid {}）", self.host, self.pid)
    }
}

/// 掴んでいる間だけ生きるロック。**落とすと解放される。**
#[derive(Debug)]
pub struct Lease {
    path: PathBuf,
    phase: Phase,
}

impl Lease {
    /// ロックを取る。既に誰かが掴んでいれば [`Error::Busy`]。
    pub(crate) fn acquire(dir: &Path, phase: Phase) -> Result<Self> {
        fs::create_dir_all(dir).map_err(|e| Error::io("ロック置き場の作成", dir, &e))?;
        let path = dir.join(phase.slug());

        // create_new は O_EXCL 相当。**作れたのは 1 人だけ**が成り立つ。
        let file = OpenOptions::new().write(true).create_new(true).open(&path);
        match file {
            Ok(mut file) => {
                let info = LeaseInfo::here();
                let line = serde_json::to_string(&info)
                    .map_err(|e| Error::corrupt(&path, e.to_string()))?;
                file.write_all(line.as_bytes())
                    .map_err(|e| Error::io("ロックの書き込み", &path, &e))?;
                Ok(Self { path, phase })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(Error::Busy { phase, holder: Box::new(read_info(&path)?) })
            }
            Err(e) => Err(Error::io("ロックの作成", &path, &e)),
        }
    }

    /// 掴んでいるフェーズ。
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// 明示的に解放する。`Drop` でも解放されるので普段は呼ばなくてよい。
    pub fn release(self) -> Result<()> {
        let path = self.path.clone();
        // Drop で二重に消さないよう、先に忘れさせる。
        std::mem::forget(self);
        fs::remove_file(&path).map_err(|e| Error::io("ロックの解放", path, &e))
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // 解放できなくても致命的ではない（次に取る人が Busy を見て判断する）。
        drop(fs::remove_file(&self.path));
    }
}

/// 残っているロックの持ち主を読む。掴まれていなければ `None`。
pub(crate) fn holder(dir: &Path, phase: Phase) -> Result<Option<LeaseInfo>> {
    let path = dir.join(phase.slug());
    if !path.exists() {
        return Ok(None);
    }
    read_info(&path).map(Some)
}

/// 残ったロックを**明示的に**壊す。
///
/// 掴んでいたホストが落ちた場合の後始末。自動ではやらない（本ファイル冒頭）。
pub(crate) fn break_lease(dir: &Path, phase: Phase) -> Result<()> {
    let path = dir.join(phase.slug());
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io("ロックの破棄", path, &e)),
    }
}

fn read_info(path: &Path) -> Result<LeaseInfo> {
    let text = fs::read_to_string(path).map_err(|e| Error::io("ロックの読み取り", path, &e))?;
    serde_json::from_str(&text).map_err(|e| Error::corrupt(path, e.to_string()))
}
