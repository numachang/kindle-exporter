//! 実機を使わずにリーダーを模す [`Browser`]。
//!
//! **これが本設計の主眼のひとつである**（ADR-0001 §6a）。派生元の `kindle_shot` は
//! 「Win32 実機依存のため対象外」としてキャプチャ層のテストを 1 件も持たなかった。
//! ブラウザを trait で切り、模擬を同梱することで、
//! 上位層（`ke-workflow` / `ke-cli`）まで実機ゼロで通せるようにする。
//!
//! 模擬は**実機で観測した癖を再現する**。位置表示を持たない書籍、
//! 「位置」で表示する書籍、先頭に戻れない書籍、送りが効かない書籍 —
//! どれも実機で起きうるので、ここで再現できなければテストの意味がない。

use ke_core::{FontControl, Observation, PageImageInfo, PageLabel, Theme};

use crate::error::{Error, Result};
use crate::{Browser, Direction, PageImage};

/// 1 ページあたりの「位置」の増分（ADR-0007 実測 5 で 41〜55）。
const LOCATIONS_PER_PAGE: u32 = 48;

/// 表示設定を変えてから、ページが再描画されるまでの時間（ADR-0007 実測 8 で 2.8 秒）。
///
/// **この間、ページ画像は DOM から消える。** ここを再現しないと
/// 「設定直後に測ろうとして落ちる」という実機の失敗を取り逃がす。
const RERENDER_MS: u64 = 2_800;

/// ページ送りのクリックが受け付けられる最小間隔（ADR-0007 実測 9）。
///
/// **これより短い間隔で押しても無視される。** 再現しないと
/// 「押し直さずに巻末と誤認して途中で打ち切る」という実機の失敗を取り逃がす。
const MIN_TURN_GAP_MS: u64 = 100;

/// 位置表示の形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Labels {
    /// `33/431ページ`。
    #[default]
    Pages,
    /// `位置9783/10167`。総数には届かない。
    Locations,
    /// 位置表示を持たない。
    None,
}

/// 実機を使わないリーダーの模擬。
#[derive(Debug, Clone)]
pub struct FakeBrowser {
    pages: u32,
    at: u32,
    first_reachable: u32,
    last_reachable: u32,
    labels: Labels,
    theme: Theme,
    font: FontControl,
    menu_open: bool,
    menu_operable: bool,
    can_turn: bool,
    ready_after: u32,
    observations: u32,
    pending_ms: u64,
    /// 仮想時計。`sleep` と観測で進む。
    clock_ms: u64,
    /// 再描画が終わる時刻。表示設定を変えるたびに先へ動く。
    image_ready_at: u64,
    /// 最後にページ送りが受け付けられた時刻。
    last_turn_at: Option<u64>,
    swallowed: u32,
    opened: Option<String>,
    grabs: Vec<u32>,
}

impl FakeBrowser {
    /// 総ページ数を決めて作る。1 ページ目から始まり、すべて正常に動く。
    #[must_use]
    pub fn with_pages(pages: u32) -> Self {
        let pages = pages.max(1);
        Self {
            pages,
            at: 1,
            first_reachable: 1,
            last_reachable: pages,
            labels: Labels::Pages,
            theme: Theme::Dark,
            font: FontControl::new(5, 13), // 実機の既定値（ADR-0007 実測 2）
            menu_open: false,
            menu_operable: true,
            can_turn: true,
            ready_after: 0,
            observations: 0,
            pending_ms: 0,
            clock_ms: 0,
            image_ready_at: 0,
            last_turn_at: None,
            swallowed: 0,
            opened: None,
            grabs: Vec::new(),
        }
    }

    /// 途中のページから始める（実機で観測した「読みかけ」の状態）。
    #[must_use]
    pub fn starting_at(mut self, page: u32) -> Self {
        self.at = page.clamp(1, self.pages);
        self
    }

    /// 位置表示を持たない書籍にする。
    #[must_use]
    pub fn without_labels(mut self) -> Self {
        self.labels = Labels::None;
        self
    }

    /// ページ番号ではなく「位置」で表示する書籍にする（ADR-0007 実測 5）。
    #[must_use]
    pub fn using_locations(mut self) -> Self {
        self.labels = Labels::Locations;
        self
    }

    /// フォントサイズの現在段と最大段を決める。
    #[must_use]
    pub fn with_font(mut self, index: u8, max: u8) -> Self {
        self.font = FontControl::new(index.min(max), max);
        self
    }

    /// 配色テーマを決める。
    #[must_use]
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// 設定メニューが開かない障害を再現する。
    #[must_use]
    pub fn with_broken_menu(mut self) -> Self {
        self.menu_operable = false;
        self
    }

    /// ページ送りが効かない障害を再現する。
    #[must_use]
    pub fn that_never_turns(mut self) -> Self {
        self.can_turn = false;
        self
    }

    /// 実際に到達できるページの範囲。先頭に戻れない書籍・
    /// 総数より手前で終わる書籍を再現する。
    #[must_use]
    pub fn reachable_pages(mut self, first: u32, last: u32) -> Self {
        self.first_reachable = first.max(1);
        self.last_reachable = last.max(self.first_reachable);
        self.at = self.at.clamp(self.first_reachable, self.last_reachable);
        self
    }

    /// 何回観測するまでページ画像が出てこないか（読み込み待ちの再現）。
    #[must_use]
    pub fn ready_after(mut self, observations: u32) -> Self {
        self.ready_after = observations;
        self
    }

    /// いまのフォント段。
    #[must_use]
    pub fn font_index(&self) -> u8 {
        self.font.index
    }

    /// いまの配色テーマ。
    #[must_use]
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// いるページ（1 起点）。
    #[must_use]
    pub fn page(&self) -> u32 {
        self.at
    }

    /// `open` で渡された URL。
    #[must_use]
    pub fn opened_url(&self) -> Option<&str> {
        self.opened.as_deref()
    }

    /// 間隔が近すぎて無視されたページ送りの回数。
    #[must_use]
    pub fn swallowed_turns(&self) -> u32 {
        self.swallowed
    }

    /// ページ画像を取り出したページの並び（撮影と実測の両方を含む）。
    #[must_use]
    pub fn grabbed_pages(&self) -> &[u32] {
        &self.grabs
    }

    /// ページ画像が使える状態になっているか。
    ///
    /// 本が描画され、かつ表示設定の変更による再描画が終わっていること。
    fn is_ready(&self) -> bool {
        self.observations > self.ready_after && self.clock_ms >= self.image_ready_at
    }

    /// 表示設定を変えた。ページは作り直されるので、しばらく画像が消える。
    fn start_rerender(&mut self) {
        self.image_ready_at = self.clock_ms.saturating_add(RERENDER_MS);
    }

    fn label(&self) -> Option<PageLabel> {
        match self.labels {
            Labels::Pages => Some(PageLabel::new(self.at, Some(self.pages))),
            // 「位置」形式は最終ページでも総数に届かない。ここが実機の肝。
            Labels::Locations => Some(PageLabel::at_location(
                self.at.saturating_mul(LOCATIONS_PER_PAGE),
                Some(self.pages.saturating_mul(LOCATIONS_PER_PAGE).saturating_add(300)),
            )),
            Labels::None => None,
        }
    }

    fn require_menu(&self, what: &'static str) -> Result<()> {
        if self.menu_open {
            return Ok(());
        }
        Err(Error::not_found(what, ["設定メニューが閉じている".to_owned()]))
    }
}

impl Browser for FakeBrowser {
    fn open(&mut self, url: &str) -> Result<()> {
        self.opened = Some(url.to_owned());
        self.observations = 0;
        Ok(())
    }

    fn observe(&mut self) -> Result<Observation> {
        self.observations = self.observations.saturating_add(1);
        self.clock_ms = self.clock_ms.saturating_add(1);
        let ready = self.is_ready();
        Ok(Observation {
            // 観測そのものにも僅かな時間がかかる。0 のままだと打ち切りが
            // 永久に来ず、テストが無限に回ってしまう。
            elapsed_ms: std::mem::take(&mut self.pending_ms).saturating_add(1),
            page: ready.then(|| self.label()).flatten(),
            image: ready.then(|| {
                PageImageInfo::ready(2199, 1692).with_source(format!("blob:page-{}", self.at))
            }),
            settings_menu_open: self.menu_open,
            font: self.menu_open.then_some(self.font),
            theme: Some(self.theme),
            // 画素/文字は OCR を持つ上位層が測る。実機と同じく None。
            metrics: None,
        })
    }

    fn set_settings_menu(&mut self, open: bool) -> Result<()> {
        if self.menu_operable {
            self.menu_open = open;
        }
        Ok(())
    }

    fn set_theme(&mut self, theme: Theme) -> Result<()> {
        self.require_menu("配色テーマの選択肢")?;
        if self.theme != theme {
            self.theme = theme;
            self.start_rerender();
        }
        Ok(())
    }

    fn set_font_size(&mut self, index: u8) -> Result<()> {
        self.require_menu("フォントサイズのスライダー")?;
        let want = self.font.clamp(index);
        if self.font.index != want {
            self.font.index = want;
            self.start_rerender();
        }
        Ok(())
    }

    fn turn_page(&mut self, direction: Direction) -> Result<()> {
        if !self.can_turn {
            return Ok(());
        }
        // 前の送りから間が空いていないクリックは無視される。
        if self.last_turn_at.is_some_and(|t| self.clock_ms < t.saturating_add(MIN_TURN_GAP_MS)) {
            self.swallowed = self.swallowed.saturating_add(1);
            return Ok(());
        }
        self.last_turn_at = Some(self.clock_ms);
        let next = match direction {
            Direction::Next => self.at.saturating_add(1),
            Direction::Prev => self.at.saturating_sub(1),
        };
        self.at = next.clamp(self.first_reachable, self.last_reachable);
        Ok(())
    }

    fn capture_page(&mut self) -> Result<PageImage> {
        if !self.is_ready() {
            return Err(Error::NoPageImage("まだ描画されていません".to_owned()));
        }
        self.grabs.push(self.at);
        // PNG に見える決定的なバイト列。ページごとに中身が変わる。
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&self.at.to_be_bytes());
        Ok(PageImage {
            png,
            width: 2199,
            height: 1692,
            source: Some(format!("blob:page-{}", self.at)),
        })
    }

    fn sleep(&mut self, ms: u32) {
        self.pending_ms = self.pending_ms.saturating_add(u64::from(ms));
        self.clock_ms = self.clock_ms.saturating_add(u64::from(ms));
    }
}
