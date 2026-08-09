//! `ke-nav` が決めた 1 手を、実際のブラウザ操作に変換する層。
//!
//! # 何をここに置き、何を置かないか
//!
//! [`Browser`] は**原始的な操作だけ**を持つ。「本を開く」「観測する」
//! 「テーマを設定する」といった、それ以上分解できない操作である。
//! [`Action`] からその原始操作への翻訳は [`apply`] 1 か所に閉じてある。
//! そのため翻訳規則は [`FakeBrowser`] 相手に 1 回テストすれば、
//! 実機側にもそのまま効く。
//!
//! **判断はここに置かない。** 「次に何をすべきか」は `ke-nav` の責務であり、
//! この層は「言われたことをやる」「見えたものを報告する」だけを行う。
//!
//! # 実機の癖（ADR-0007。忘れると必ず詰まる）
//!
//! - **JS は isolated world で評価する。** Amazon が `window.fetch` を
//!   差し替えているため、素の文脈では canvas 経由の取り出しが失敗する
//! - **`element.click()` は効かない。** `Input.dispatchMouseEvent` を使う
//! - **矢印キーはページ送りにならない。** フォーカスされた位置シークバーが
//!   1 目盛り動くだけだった。送りは chevron ボタンを押す
//! - **縦書き書籍では左の chevron が「次」である。** 左右で決めてはいけない
//! - `Page.createIsolatedWorld` のパラメータ名 `grantUniveralAccess` は
//!   **プロトコル側の綴り間違い**であって、こちらの typo ではない

#![forbid(unsafe_code)]

mod client;
mod error;
mod fake;
mod js;
mod reader;

pub use client::Endpoint;
pub use error::{Error, Result};
pub use fake::FakeBrowser;
pub use reader::CdpBrowser;

use ke_core::{Action, Observation, PageLabel, Theme};

/// ページを送る向き。
///
/// **画面の左右ではなく読み進む向き**を指す。縦書き書籍では
/// [`Direction::Next`] が画面左のボタンに対応する（ADR-0007 実測 4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 読み進む方向。
    Next,
    /// 読み戻る方向。
    Prev,
}

/// 取り出したページ画像。
///
/// Cloud Reader はページをサーバ側でレンダリング済みの画像として配信するため、
/// これは**画面の複製ではなく配信された原本**である（ADR-0004 実測 3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    /// PNG のバイト列。
    pub png: Vec<u8>,
    /// 原寸の幅（px）。
    pub width: u32,
    /// 原寸の高さ（px）。
    pub height: u32,
    /// 画像の出所（blob URL）。ページごとに変わる。
    pub source: Option<String>,
}

/// [`apply`] が生んだ副産物。
///
/// [`Browser`] は保存も OCR もしない。作ったものを呼び出し側に手渡し、
/// どこへ置くか・何にかけるかは上位層（`ke-store` / `ke-workflow`）が決める。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// 観測以外に残るものはない。
    Nothing,
    /// 保存すべきページ画像。
    Captured {
        /// 対応づける位置。
        label: PageLabel,
        /// 取り出した画像。
        image: PageImage,
    },
    /// 実測（画素/文字の測定）にかけるべきページ画像。
    ToMeasure(PageImage),
    /// 終端に達した。ブラウザは何もしていない。
    Terminal,
}

/// ブラウザに対してできること。
///
/// 実装は 2 つある。[`CdpBrowser`] が実機で、[`FakeBrowser`] が模擬である。
/// **模擬を同梱するのは、上位層を実機ゼロでテストするためである**（ADR-0001 §6a）。
pub trait Browser {
    /// 指定 URL を開く。
    fn open(&mut self, url: &str) -> Result<()>;

    /// いま観測できることをまとめて取る。
    fn observe(&mut self) -> Result<Observation>;

    /// 設定メニューを開く・閉じる。**既にその状態なら何もしない。**
    ///
    /// トグル式のボタンなので、状態を確かめずに押すと逆に働く。
    fn set_settings_menu(&mut self, open: bool) -> Result<()>;

    /// 配色テーマを設定する。設定メニューが開いている必要がある。
    fn set_theme(&mut self, theme: Theme) -> Result<()>;

    /// フォントサイズを指定の段にする。設定メニューが開いている必要がある。
    ///
    /// 現在段を読めるので**この操作は冪等である**（ADR-0007 決定 2）。
    fn set_font_size(&mut self, index: u8) -> Result<()>;

    /// ページを送る・戻す。
    fn turn_page(&mut self, direction: Direction) -> Result<()>;

    /// いま表示しているページ画像を PNG で取り出す。
    fn capture_page(&mut self) -> Result<PageImage>;

    /// 指定時間待つ。模擬では実際には待たない。
    fn sleep(&mut self, ms: u32);
}

/// `ke-nav` が決めた 1 手を実行する。
///
/// [`Action`] の全変種をここで受け止める。**翻訳規則はここだけにある**ので、
/// 実装を増やしても分岐が散らばらない。
pub fn apply<B: Browser + ?Sized>(browser: &mut B, action: &Action) -> Result<Effect> {
    match action {
        Action::MeasurePage => return browser.capture_page().map(Effect::ToMeasure),
        Action::CapturePage { label } => {
            let image = browser.capture_page()?;
            return Ok(Effect::Captured { label: label.clone(), image });
        }
        Action::Done(_) | Action::Fail(_) => return Ok(Effect::Terminal),
        other => apply_control(browser, other)?,
    }
    Ok(Effect::Nothing)
}

/// 副産物を生まない操作。
fn apply_control<B: Browser + ?Sized>(browser: &mut B, action: &Action) -> Result<()> {
    match action {
        Action::OpenBook { url } => browser.open(url),
        Action::Wait { ms, .. } => {
            browser.sleep(*ms);
            Ok(())
        }
        Action::OpenSettingsMenu => browser.set_settings_menu(true),
        Action::CloseSettingsMenu => browser.set_settings_menu(false),
        Action::SetTheme(theme) => browser.set_theme(*theme),
        Action::SetFontSize { index } => browser.set_font_size(*index),
        Action::PressNext => browser.turn_page(Direction::Next),
        Action::PressPrev => browser.turn_page(Direction::Prev),
        // 副産物のある行動と終端は apply が先に処理している。
        _ => Ok(()),
    }
}
