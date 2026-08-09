//! 実機の Kindle Cloud Reader を CDP で駆動する [`Browser`] 実装。

use std::thread::sleep;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ke_core::{FontControl, Observation, PageImageInfo, PageLabel, Theme, TurnControls};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{Cdp, Endpoint};
use crate::error::{Error, Result};
use crate::{Browser, Direction, PageImage, js};

/// フォント段を合わせるために押す上限。実機は 14 段なので十分な余裕がある。
const FONT_PRESS_LIMIT: u8 = 64;
/// 1 回押したあと、`value` 属性が変わるのを待つ上限。
const FONT_SETTLE: Duration = Duration::from_millis(1_200);
/// 属性のポーリング間隔。
const POLL: Duration = Duration::from_millis(20);
/// ポインタを動かしてから chevron が描かれるまでの待ち。
const HOVER_SETTLE: Duration = Duration::from_millis(150);
/// chevron を出させるために動かす位置（viewport に対する割合）。実測 5% / 95% / 50%。
const HOVER_SIDES: [f64; 2] = [0.05, 0.95];
const HOVER_MIDDLE: f64 = 0.5;

/// CDP 経由で実機のリーダーを操作する。
#[derive(Debug)]
pub struct CdpBrowser {
    cdp: Cdp,
    /// 組み込みが汚染されていない JS 文脈。ページ遷移で無効になるので作り直す。
    context: Option<i64>,
    worlds: u32,
    /// 直前の観測からの経過時間を測るための時計。
    last_observed: Instant,
}

#[derive(Debug, Deserialize)]
struct RawImage {
    source: Option<String>,
    #[serde(rename = "naturalWidth")]
    natural_width: u32,
    #[serde(rename = "naturalHeight")]
    natural_height: u32,
    complete: bool,
}

#[derive(Debug, Deserialize)]
struct RawObservation {
    label: Option<String>,
    image: Option<RawImage>,
    #[serde(rename = "settingsMenuOpen")]
    settings_menu_open: bool,
    font: Option<FontControl>,
    theme: Option<String>,
    #[serde(default)]
    chevrons: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawCapture {
    error: Option<String>,
    #[serde(rename = "dataUrl")]
    data_url: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawButton {
    aria: String,
    x: f64,
    y: f64,
}

#[derive(Debug, Deserialize)]
struct RawChevron {
    aria: String,
    x: f64,
    y: f64,
    visible: bool,
}

#[derive(Debug, Deserialize)]
struct RawChevrons {
    list: Vec<RawChevron>,
    viewport: (f64, f64),
}

/// 送りの操作子がいまどうなっているか。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Chevron {
    /// DOM から消えている＝その方向へは進めない（＝端にいる）。
    Missing,
    /// DOM にはあるが描かれていない。ポインタを近づければ出る。
    Hidden,
    /// 押せる位置にある。
    At(f64, f64),
}

impl CdpBrowser {
    /// 既定のポート（9222）で開いている Cloud Reader のタブに繋ぐ。
    pub fn connect() -> Result<Self> {
        Self::connect_to(&Endpoint::discover(9222, "read.amazon")?)
    }

    /// 接続先を指定して繋ぐ。
    pub fn connect_to(endpoint: &Endpoint) -> Result<Self> {
        Ok(Self {
            cdp: Cdp::attach(endpoint)?,
            context: None,
            worlds: 0,
            last_observed: Instant::now(),
        })
    }

    /// 組み込みが汚染されていない JS 文脈を用意する（ADR-0004 実測 3）。
    fn world(&mut self) -> Result<i64> {
        if let Some(id) = self.context {
            return Ok(id);
        }
        let tree = self.cdp.send("Page.getFrameTree", json!({}))?;
        let frame = tree
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Unexpected(format!("frameTree の形が想定外: {tree}")))?
            .to_owned();
        self.worlds = self.worlds.saturating_add(1);
        let world = self.cdp.send(
            "Page.createIsolatedWorld",
            // `grantUniveralAccess` は CDP 側の綴り間違い。直すと動かない。
            json!({ "frameId": frame, "worldName": format!("ke{}", self.worlds),
                    "grantUniveralAccess": true }),
        )?;
        let id = world.get("executionContextId").and_then(Value::as_i64).ok_or_else(|| {
            Error::Unexpected(format!("executionContextId がありません: {world}"))
        })?;
        self.context = Some(id);
        Ok(id)
    }

    /// JS を評価して、返ってきた JSON 文字列を型に落とす。
    ///
    /// 文脈が失効していたら 1 度だけ作り直して再試行する
    /// （ページ遷移や再描画で isolated world は消える）。
    fn eval<T: serde::de::DeserializeOwned>(&mut self, expr: &str, awaited: bool) -> Result<T> {
        let raw = match self.eval_raw(expr, awaited) {
            Err(Error::Protocol { .. } | Error::Script(_)) => {
                self.context = None;
                self.eval_raw(expr, awaited)?
            }
            other => other?,
        };
        serde_json::from_str(&raw).map_err(|e| {
            Error::Unexpected(format!("{e}: {}", raw.chars().take(200).collect::<String>()))
        })
    }

    fn eval_raw(&mut self, expr: &str, awaited: bool) -> Result<String> {
        let context = self.world()?;
        let result = self.cdp.send(
            "Runtime.evaluate",
            json!({ "expression": expr, "contextId": context,
                    "returnByValue": true, "awaitPromise": awaited }),
        )?;
        if let Some(details) = result.get("exceptionDetails") {
            return Err(Error::Script(details.to_string()));
        }
        match result.pointer("/result/value").and_then(Value::as_str) {
            Some(text) => Ok(text.to_owned()),
            None => Err(Error::Unexpected(format!("JSON 文字列が返りませんでした: {result}"))),
        }
    }

    /// 座標をクリックする。`element.click()` は効かない（ADR-0007 実測 3）。
    fn click_at(&mut self, x: f64, y: f64) -> Result<()> {
        for kind in ["mousePressed", "mouseReleased"] {
            self.cdp.send(
                "Input.dispatchMouseEvent",
                json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
            )?;
        }
        Ok(())
    }

    /// セレクタで指す要素の中心をクリックする。
    fn click(&mut self, selector: &'static str, what: &'static str) -> Result<()> {
        let point: Option<(f64, f64)> = self.eval(&js::center_of(selector), false)?;
        let (x, y) = point.ok_or_else(|| Error::not_found(what, [selector.to_owned()]))?;
        self.click_at(x, y)
    }

    /// `aria-label` が一致する表示中のボタンをクリックする。
    ///
    /// ページ送りと設定メニューは `aria-label` でしか見分けられない。
    /// **一致しなければ推測せずに失敗させる**（ADR-0007 決定 3）。
    fn click_labelled(&mut self, labels: &[&str], what: &'static str) -> Result<()> {
        let buttons: Vec<RawButton> = self.eval(js::LABELLED_BUTTONS, false)?;
        let hit = buttons.iter().find(|b| labels.contains(&b.aria.as_str()));
        match hit {
            Some(b) => self.click_at(b.x, b.y),
            None => Err(Error::not_found(what, buttons.iter().map(|b| b.aria.clone()))),
        }
    }

    /// 指定方向の chevron がいまどうなっているか。
    fn chevron(&mut self, labels: &[&str]) -> Result<(Chevron, (f64, f64))> {
        let found: RawChevrons = self.eval(js::CHEVRONS, false)?;
        let hit = found.list.iter().find(|c| labels.contains(&c.aria.as_str()));
        let state = match hit {
            None => Chevron::Missing,
            Some(c) if c.visible => Chevron::At(c.x, c.y),
            Some(_) => Chevron::Hidden,
        };
        Ok((state, found.viewport))
    }

    /// ポインタを動かして、隠れている chevron を出させる。
    fn hover(&mut self, x: f64, y: f64) -> Result<()> {
        self.cdp
            .send("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": y }))?;
        sleep(HOVER_SETTLE);
        Ok(())
    }

    /// フォントサイズの現在段。設定メニューが閉じていれば `None`。
    fn font(&mut self) -> Result<Option<FontControl>> {
        self.eval(&js::font_only(), false)
    }

    /// 1 段押したあと、`value` 属性が実際に変わるまで待つ。
    fn wait_for_font_change(&mut self, before: u8) -> Result<bool> {
        let deadline = Instant::now() + FONT_SETTLE;
        while Instant::now() < deadline {
            if self.font()?.is_some_and(|f| f.index != before) {
                return Ok(true);
            }
            sleep(POLL);
        }
        Ok(false)
    }

    fn menu_is_open(&mut self) -> Result<bool> {
        Ok(self.eval::<RawObservation>(&js::observe(), false)?.settings_menu_open)
    }
}

impl Browser for CdpBrowser {
    fn open(&mut self, url: &str) -> Result<()> {
        self.cdp.send("Page.navigate", json!({ "url": url }))?;
        // 遷移すると isolated world は消える。次の評価で作り直す。
        self.context = None;
        Ok(())
    }

    fn observe(&mut self) -> Result<Observation> {
        let raw: RawObservation = self.eval(&js::observe(), false)?;
        let elapsed = self.last_observed.elapsed();
        self.last_observed = Instant::now();
        Ok(Observation {
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            page: raw.label.as_deref().and_then(PageLabel::parse),
            image: raw.image.map(|i| PageImageInfo {
                source: i.source,
                natural_width: i.natural_width,
                natural_height: i.natural_height,
                complete: i.complete,
            }),
            settings_menu_open: raw.settings_menu_open,
            font: raw.font,
            theme: raw.theme.as_deref().and_then(parse_theme),
            turn_controls: turn_controls(&raw.chevrons),
            // 画素/文字は OCR を持つ上位層が測る。ここでは分からない。
            metrics: None,
        })
    }

    fn set_settings_menu(&mut self, open: bool) -> Result<()> {
        // トグルなので、状態を確かめずに押すと逆に働く。
        if self.menu_is_open()? == open {
            return Ok(());
        }
        if open {
            return self.click_labelled(js::SETTINGS_LABELS, "設定メニューを開くボタン");
        }
        // 閉じるボタンは表示言語に依存しないので、あればそちらを使う。
        match self.click(js::MENU_CLOSE, "設定メニューを閉じるボタン") {
            Err(Error::ElementNotFound { .. }) => {
                self.click_labelled(js::SETTINGS_LABELS, "設定メニューを閉じるボタン")
            }
            other => other,
        }
    }

    fn set_theme(&mut self, theme: Theme) -> Result<()> {
        self.click(js::theme_selector(theme), "配色テーマの選択肢")
    }

    fn set_font_size(&mut self, index: u8) -> Result<()> {
        let mut last = None;
        for _ in 0..FONT_PRESS_LIMIT {
            let Some(font) = self.font()? else {
                return Err(Error::not_found("フォントサイズのスライダー", []));
            };
            last = Some(font.index);
            let want = font.clamp(index);
            if font.index == want {
                return Ok(());
            }
            let bigger = font.index < want;
            self.click(
                if bigger { js::FONT_BIGGER } else { js::FONT_SMALLER },
                "フォントサイズの増減ボタン",
            )?;
            self.wait_for_font_change(font.index)?;
        }
        Err(Error::SettingRejected {
            what: "フォントサイズ",
            wanted: index.to_string(),
            got: last.map_or_else(|| "不明".to_owned(), |i| i.to_string()),
        })
    }

    /// ページを送る。
    ///
    /// リーダーは**ポインタのある側の chevron しか描かない**（ADR-0007 実測 11）。
    /// 描かれていなければ左右にポインタを動かして出させる。
    /// **DOM から消えている場合は端にいる**ので、探し回らずに失敗させる。
    fn turn_page(&mut self, direction: Direction) -> Result<()> {
        let (labels, what) = match direction {
            Direction::Next => (js::NEXT_LABELS, "「次のページ」ボタン"),
            Direction::Prev => (js::PREV_LABELS, "「前のページ」ボタン"),
        };
        let (state, viewport) = self.chevron(labels)?;
        match state {
            Chevron::At(x, y) => return self.click_at(x, y),
            Chevron::Missing => {
                return Err(Error::not_found(what, ["端に達しています".to_owned()]));
            }
            Chevron::Hidden => {}
        }
        // どちら側にあるかは矩形が無いと分からないので、両側を試す。
        for ratio in HOVER_SIDES {
            let (x, y) = (viewport.0 * ratio, viewport.1 * HOVER_MIDDLE);
            self.hover(x, y)?;
            if let (Chevron::At(cx, cy), _) = self.chevron(labels)? {
                return self.click_at(cx, cy);
            }
        }
        Err(Error::not_found(what, ["DOM にはあるが描かれていません".to_owned()]))
    }

    fn capture_page(&mut self) -> Result<PageImage> {
        let raw: RawCapture = self.eval(js::CAPTURE, true)?;
        if let Some(message) = raw.error {
            return Err(Error::NoPageImage(message));
        }
        let (Some(data_url), Some(width), Some(height)) = (raw.data_url, raw.width, raw.height)
        else {
            return Err(Error::NoPageImage("応答に画像が入っていません".to_owned()));
        };
        let encoded = data_url
            .split_once(',')
            .map(|(_, b)| b)
            .ok_or_else(|| Error::NoPageImage("data URL の形が想定外です".to_owned()))?;
        let png = BASE64
            .decode(encoded)
            .map_err(|e| Error::NoPageImage(format!("base64 を解けません: {e}")))?;
        Ok(PageImage { png, width, height, source: raw.source })
    }

    fn sleep(&mut self, ms: u32) {
        sleep(Duration::from_millis(u64::from(ms)));
    }
}

/// chevron の**存在**から、どちらへ動けるかを読む（ADR-0007 実測 11）。
///
/// **巻末では「次のページ」が、先頭では「前のページ」が DOM から消える。**
/// 描かれているかどうかは見ない — ポインタが反対側にあるだけで矩形は 0 になり、
/// それを端と取り違えると本の途中で撮影を終えてしまう。
/// 1 つも無ければ本がまだ描画されていないので `None`（判断しない）。
fn turn_controls(chevrons: &[String]) -> Option<TurnControls> {
    if chevrons.is_empty() {
        return None;
    }
    let has = |labels: &[&str]| chevrons.iter().any(|c| labels.contains(&c.as_str()));
    Some(TurnControls { next: has(js::NEXT_LABELS), prev: has(js::PREV_LABELS) })
}

/// リーダーが返すテーマ名を型に落とす。
///
/// 設定メニューの radio は `White` / `Dark` / `Sepia` / `Green` を返し、
/// `#kr-renderer` の class は明色を **`default`** と呼ぶ（ADR-0007 実測 4）。
fn parse_theme(name: &str) -> Option<Theme> {
    match name.to_ascii_lowercase().as_str() {
        "white" | "default" => Some(Theme::White),
        "dark" => Some(Theme::Dark),
        "sepia" => Some(Theme::Sepia),
        "green" => Some(Theme::Green),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 明色テーマの class 名は `--white` ではない。ここを取り違えると
    /// テーマが正しいのに何度も設定し直すことになる。
    /// 巻末・先頭の判定は chevron の有無で行う。
    #[test]
    fn reads_which_way_the_reader_can_still_turn() {
        let both = ["次のページ".to_owned(), "前のページ".to_owned()];
        assert_eq!(turn_controls(&both), Some(TurnControls::both()));
        assert_eq!(turn_controls(&both[1..]), Some(TurnControls::at_end()));
        assert_eq!(turn_controls(&both[..1]), Some(TurnControls::at_start()));
        // 本がまだ描画されていない。端だと決めつけない。
        assert_eq!(turn_controls(&[]), None);
    }

    #[test]
    fn reads_the_light_theme_under_both_of_its_names() {
        assert_eq!(parse_theme("White"), Some(Theme::White));
        assert_eq!(parse_theme("default"), Some(Theme::White));
        assert_eq!(parse_theme("dark"), Some(Theme::Dark));
        assert_eq!(parse_theme("Sepia"), Some(Theme::Sepia));
        assert_eq!(parse_theme("なにか未知のもの"), None);
    }
}
