//! 蔵書アーティファクトの置き場所。
//!
//! ADR-0001 §3 の配置をそのまま実装する。
//!
//! ```text
//! library/<ASIN>/
//!   manifest.json          BookSpec
//!   events.jsonl           追記専用のイベントログ（唯一の真実）
//!   pages/raw/0001.png     capture の出力
//!   text/0001.json         ocr の出力
//!   out/<title>.pdf        assemble の出力
//!   .lease/capture         フェーズの排他ロック
//! ```
//!
//! # なぜデータベースを置かないか
//!
//! **共有ストレージ（SMB / Syncthing）上では SQLite のロックが壊れる。**
//! 書籍ごとの追記専用 JSONL なら衝突せず、マージでき、オフラインでも進む。
//! ローカルの索引を作る場合も、[`events`] から再構築できるものに限ること。
//!
//! この配置の帰結として、**Mac が N+1 冊目を撮っている間に Windows が
//! N 冊目を OCR できる。** 片方が落ちても、電源を切っても、そこから再開できる。

#![forbid(unsafe_code)]

mod error;
mod events;
mod lease;

pub use error::{Error, Result};
pub use events::{Event, Progress, Record};
pub use lease::{Lease, LeaseInfo};

use std::fs::{self, OpenOptions};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ke_core::{Asin, BookSpec, Phase};

/// 蔵書全体の置き場所。
#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    /// 置き場所を指定して開く。ディレクトリはまだ無くてよい。
    #[must_use]
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 置き場所のパス。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 1 冊分を指す。**この時点ではまだ何も作らない。**
    #[must_use]
    pub fn book(&self, asin: &Asin) -> Book {
        Book { dir: self.root.join(asin.as_str()) }
    }

    /// 登録済みの書籍。`manifest.json` を持つディレクトリだけを数える。
    pub fn books(&self) -> Result<Vec<Asin>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let entries =
            fs::read_dir(&self.root).map_err(|e| Error::io("蔵書の一覧", &self.root, &e))?;
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| Error::io("蔵書の一覧", &self.root, &e))?;
            if !entry.path().join("manifest.json").exists() {
                continue;
            }
            if let Some(asin) = entry.file_name().to_str().and_then(|n| Asin::new(n).ok()) {
                found.push(asin);
            }
        }
        found.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(found)
    }
}

/// 1 冊分のアーティファクト。
#[derive(Debug, Clone)]
pub struct Book {
    dir: PathBuf,
}

impl Book {
    /// この書籍のディレクトリ。
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 登録済みか（`manifest.json` があるか）。
    #[must_use]
    pub fn exists(&self) -> bool {
        self.manifest_path().exists()
    }

    /// 書籍を登録する。既にあれば `manifest.json` を書き直すだけで、
    /// **イベントログとページ画像には触らない**（再登録で進捗を失わない）。
    pub fn register(&self, spec: &BookSpec) -> Result<()> {
        let first_time = !self.exists();
        for dir in [&self.dir, &self.pages_dir(), &self.text_dir(), &self.out_dir()] {
            fs::create_dir_all(dir).map_err(|e| Error::io("書籍ディレクトリの作成", dir, &e))?;
        }
        let json = serde_json::to_string_pretty(spec)
            .map_err(|e| Error::corrupt(self.manifest_path(), e.to_string()))?;
        fs::write(self.manifest_path(), json)
            .map_err(|e| Error::io("manifest.json の書き込み", self.manifest_path(), &e))?;
        if first_time {
            self.append(Event::BookRegistered)?;
        }
        Ok(())
    }

    /// 登録内容を読む。
    pub fn manifest(&self) -> Result<BookSpec> {
        let path = self.manifest_path();
        let text = fs::read_to_string(&path)
            .map_err(|e| Error::io("manifest.json の読み取り", &path, &e))?;
        serde_json::from_str(&text).map_err(|e| Error::corrupt(path, e.to_string()))
    }

    /// イベントを 1 件追記する。
    ///
    /// **追記しかしない。** 書き換えを許すと、複数ホストが同じ書籍を
    /// 触ったときに履歴が失われる。
    pub fn append(&self, event: Event) -> Result<()> {
        let path = self.events_path();
        let line = serde_json::to_string(&Record::here(event))
            .map_err(|e| Error::corrupt(&path, e.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| Error::io("イベントログを開く", &path, &e))?;
        writeln!(file, "{line}").map_err(|e| Error::io("イベントログへの追記", &path, &e))
    }

    /// 記録された全イベント。
    pub fn events(&self) -> Result<Vec<Record>> {
        let path = self.events_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&path).map_err(|e| Error::io("イベントログを開く", &path, &e))?;
        events::read_records(BufReader::new(file), &path)
    }

    /// いまどこまで進んでいるか。**再開はこれを見て決める。**
    pub fn progress(&self) -> Result<Progress> {
        Ok(Progress::from_records(&self.events()?))
    }

    /// ページ画像を保存し、イベントに残す。
    ///
    /// **一時ファイルに書いてから改名する。** 途中で電源が落ちても、
    /// 中途半端な PNG が「撮れたページ」として残らないようにするため。
    pub fn save_page(
        &self,
        index: u32,
        png: &[u8],
        label: Option<ke_core::PageLabel>,
    ) -> Result<PathBuf> {
        let dir = self.pages_dir();
        fs::create_dir_all(&dir).map_err(|e| Error::io("ページ置き場の作成", &dir, &e))?;

        let path = self.page_path(index);
        let partial = path.with_extension("png.part");
        fs::write(&partial, png).map_err(|e| Error::io("ページ画像の書き込み", &partial, &e))?;
        fs::rename(&partial, &path).map_err(|e| Error::io("ページ画像の確定", &path, &e))?;

        let bytes = u64::try_from(png.len()).unwrap_or(u64::MAX);
        self.append(Event::PageCaptured { index, label, bytes })?;
        Ok(path)
    }

    /// ページ画像のパス。
    #[must_use]
    pub fn page_path(&self, index: u32) -> PathBuf {
        self.pages_dir().join(format!("{index:04}.png"))
    }

    /// フェーズの排他ロックを取る。落とすと解放される。
    pub fn lease(&self, phase: Phase) -> Result<Lease> {
        Lease::acquire(&self.lease_dir(), phase)
    }

    /// そのフェーズを掴んでいる相手。掴まれていなければ `None`。
    pub fn lease_holder(&self, phase: Phase) -> Result<Option<LeaseInfo>> {
        lease::holder(&self.lease_dir(), phase)
    }

    /// 残ったロックを**明示的に**壊す。
    ///
    /// 掴んでいたホストが落ちた場合の後始末。自動でやってはいけない
    /// （生きている相手と二重に走る）。
    pub fn break_lease(&self, phase: Phase) -> Result<()> {
        lease::break_lease(&self.lease_dir(), phase)
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.json")
    }

    fn events_path(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    fn pages_dir(&self) -> PathBuf {
        self.dir.join("pages").join("raw")
    }

    fn text_dir(&self) -> PathBuf {
        self.dir.join("text")
    }

    fn out_dir(&self) -> PathBuf {
        self.dir.join("out")
    }

    fn lease_dir(&self) -> PathBuf {
        self.dir.join(".lease")
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// このホストの名前。取れなければ `unknown`。
///
/// 依存を増やさないため環境変数から取る。**取れなくても動作は変わらない**
/// （イベントログの読みやすさが落ちるだけ）。
fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}
