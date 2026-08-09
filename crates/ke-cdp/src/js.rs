//! リーダーの中で評価する JS。
//!
//! **すべて isolated world で評価する。** Amazon が `window.fetch` などの
//! 組み込みを差し替えているため、素の文脈では canvas 経由の取り出しが
//! `TypeError: Failed to fetch` で失敗する（ADR-0004 実測 3）。
//!
//! セレクタは実機の DOM から起こしてある（ADR-0007）。
//! 表示言語に依存しないもの（id・class）を優先し、
//! `aria-label` に頼るのは他に手が無いところだけにしている。

use ke_core::Theme;

/// 設定メニューを閉じるボタン。表示言語に依存しない。
pub const MENU_CLOSE: &str = ".side-menu-close-button";
/// フォントサイズを 1 段上げるボタン。
pub const FONT_BIGGER: &str = ".font-size-slider__label--end";
/// フォントサイズを 1 段下げるボタン。
pub const FONT_SMALLER: &str = ".font-size-slider__label--start";

/// 設定メニューを開くボタンの `aria-label`（ADR-0007 実測 3）。
///
/// このボタンは他のツールバーボタンと class を共有しており、
/// **`aria-label` 以外に見分ける手掛かりが無い。**
pub const SETTINGS_LABELS: &[&str] = &["リーダー設定", "Reader settings", "Reader Settings"];
/// 「次のページ」ボタンの `aria-label`。
///
/// **左右で決めてはいけない。** 縦書き書籍では左が「次」である（ADR-0007 実測 4）。
pub const NEXT_LABELS: &[&str] = &["次のページ", "Next Page", "Next page"];
/// 「前のページ」ボタンの `aria-label`。
pub const PREV_LABELS: &[&str] = &["前のページ", "Previous Page", "Previous page"];

/// 配色テーマの radio の CSS セレクタ。id は表示言語に依存しない。
#[must_use]
pub fn theme_selector(theme: Theme) -> &'static str {
    match theme {
        Theme::White => "#theme-White",
        Theme::Dark => "#theme-Dark",
        Theme::Sepia => "#theme-Sepia",
        Theme::Green => "#theme-Green",
    }
}

/// 要素の中心座標を返す式。見つからない・表示されていなければ `null`。
#[must_use]
pub fn center_of(selector: &str) -> String {
    // セレクタは JSON 文字列として埋める（引用符やバックスラッシュを自前で
    // 組み立てると壊れるため）。
    let quoted = serde_json::Value::String(selector.to_owned()).to_string();
    format!(
        "(() => {{ const e = document.querySelector({quoted}); if (!e) return 'null'; \
         const r = e.getBoundingClientRect(); if (!r.width || !r.height) return 'null'; \
         return JSON.stringify([r.x + r.width / 2, r.y + r.height / 2]); }})()"
    )
}

/// 観測できることを 1 回の evaluate でまとめて取る（実測 0.4ms）。
pub const OBSERVE: &str = r##"
(() => {
  const q = (s) => document.querySelector(s);
  const menu = q('ion-menu');

  // フッターには「読書速度を学習中...」など位置以外の .text-div もあるので、
  // 「数字/数字」を含むものだけを拾う。
  const label = [...document.querySelectorAll('.text-div')]
    .map((e) => (e.textContent || '').trim())
    .find((t) => /[0-9]+\s*\/\s*[0-9]+/.test(t)) || null;

  const img = q('.kg-full-page-img img') || q('#kr-renderer img');
  const image = img ? {
    source: img.currentSrc || img.src || null,
    naturalWidth: img.naturalWidth || 0,
    naturalHeight: img.naturalHeight || 0,
    complete: !!img.complete,
  } : null;

  const theme = (() => {
    const on = [...document.querySelectorAll('.theme-selector [role="radio"]')]
      .find((e) => e.getAttribute('aria-checked') === 'true');
    if (on) return on.getAttribute('value');
    // メニューが閉じているときの控え。明色テーマの class は --white ではなく --default。
    const m = /kg-client-theme--([A-Za-z]+)/.exec((q('#kr-renderer') || {}).className || '');
    return m ? m[1] : null;
  })();

  // 巻末では「次のページ」、先頭では「前のページ」が **DOM から消える**。
  // 一方、ポインタが反対側にあるだけの場合は DOM に残って矩形が 0 になる。
  // 端の判定は**存在するかどうか**で行う（ADR-0007 実測 11）。
  const chevrons = [...document.querySelectorAll('button')]
    .filter((b) => (b.id || '').startsWith('kr-chevron'))
    .map((b) => b.getAttribute('aria-label'));

  return JSON.stringify({
    label,
    image,
    settingsMenuOpen: !!menu && menu.classList.contains('show-menu'),
    font: FONT,
    theme,
    chevrons,
  });
})()
"##;

/// フォントサイズの現在段と最大段。`value` は**属性としてしか読めない**（ADR-0007 実測 2）。
///
/// `#kr-scrubber-bar`（位置シークバー）も `ion-range` なので必ず除外する。
pub const FONT: &str = r##"
(() => {
  const menu = document.querySelector('ion-menu');
  if (!menu) return null;
  const range = menu.querySelector('ion-range.font-size-slider')
    || [...menu.querySelectorAll('ion-range')].find((e) => e.id !== 'kr-scrubber-bar');
  if (!range) return null;
  const index = parseInt(range.getAttribute('value'), 10);
  const max = parseInt(range.getAttribute('max'), 10);
  return Number.isInteger(index) && Number.isInteger(max) ? { index, max } : null;
})()
"##;

/// フォントサイズだけを読む式（設定中のポーリング用）。
#[must_use]
pub fn font_only() -> String {
    format!("(() => JSON.stringify({FONT}))()")
}

/// 観測式（`OBSERVE` の `FONT` を実体に差し替えたもの）。
#[must_use]
pub fn observe() -> String {
    OBSERVE.replace("FONT", FONT)
}

/// ページ画像を原寸の PNG として取り出す（ADR-0004 実測 3。実測 17ms）。
///
/// blob は same-origin なので canvas は汚染されない。
pub const CAPTURE: &str = r##"
(async () => {
  const img = document.querySelector('.kg-full-page-img img')
    || document.querySelector('#kr-renderer img');
  if (!img) return JSON.stringify({ error: 'ページ画像の <img> がありません' });
  if (!img.complete || !img.naturalWidth) {
    await new Promise((r) => { img.onload = r; setTimeout(r, 5000); });
  }
  if (!img.complete || !img.naturalWidth) {
    return JSON.stringify({ error: 'ページ画像が読み込み中のままです' });
  }
  const c = document.createElement('canvas');
  c.width = img.naturalWidth;
  c.height = img.naturalHeight;
  c.getContext('2d').drawImage(img, 0, 0);
  let dataUrl = null;
  try {
    dataUrl = c.toDataURL('image/png');
  } catch (e) {
    return JSON.stringify({ error: 'canvas が汚染されています: ' + String(e) });
  }
  return JSON.stringify({
    dataUrl,
    width: img.naturalWidth,
    height: img.naturalHeight,
    source: img.currentSrc || img.src || null,
  });
})()
"##;

/// ページ送りの chevron を、存在と矩形の両方つきで列挙する。
///
/// **存在するのに矩形が 0 のことがある。** リーダーはポインタのある側の
/// chevron しか描画しない（ADR-0007 実測 11）。押すには先に反対側へ
/// ポインタを動かして出させる必要がある。
pub const CHEVRONS: &str = r##"
(() => {
  const list = [...document.querySelectorAll('button')]
    .filter((b) => (b.id || '').startsWith('kr-chevron'))
    .map((b) => {
      const r = b.getBoundingClientRect();
      return { aria: b.getAttribute('aria-label'), x: r.x + r.width / 2, y: r.y + r.height / 2,
               visible: r.width > 0 && r.height > 0 };
    });
  return JSON.stringify({ list, viewport: [innerWidth, innerHeight] });
})()
"##;

/// `aria-label` を持つ表示中のボタンを、中心座標つきで列挙する。
///
/// ページ送りと設定メニューの操作子は `aria-label` でしか見分けられないので、
/// 「見つからない」ときに**何が見えていたか**を報告できるよう一覧で取る。
pub const LABELLED_BUTTONS: &str = r##"
(() => JSON.stringify([...document.querySelectorAll('ion-button, button')]
  .map((b) => {
    const r = b.getBoundingClientRect();
    return { aria: b.getAttribute('aria-label'), x: r.x + r.width / 2, y: r.y + r.height / 2,
             visible: r.width > 0 && r.height > 0 };
  })
  .filter((b) => b.aria && b.visible)))()
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_selectors_so_they_cannot_break_the_expression() {
        let js = center_of("#theme-White");
        assert!(js.contains(r##"querySelector("#theme-White")"##), "{js}");
        // 引用符を含むセレクタでも式が壊れない
        let tricky = center_of(r#"[aria-label="次のページ"]"#);
        assert!(tricky.contains(r#"\"次のページ\""#), "{tricky}");
    }

    #[test]
    fn theme_selectors_use_language_independent_ids() {
        assert_eq!(theme_selector(Theme::White), "#theme-White");
        assert_eq!(theme_selector(Theme::Dark), "#theme-Dark");
    }

    /// 観測式にフォント読み取りが埋め込まれていること（差し替えに失敗すると
    /// `font: FONT` という壊れた JS を送ってしまう）。
    #[test]
    fn the_observe_expression_embeds_the_font_probe() {
        let js = observe();
        assert!(!js.contains("font: FONT"), "FONT を差し替えられていない");
        assert!(js.contains("kr-scrubber-bar"), "シークバーの除外が消えている");
    }
}
