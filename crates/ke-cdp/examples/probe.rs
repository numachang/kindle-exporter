//! 実機のリーダーに繋いで `ke-cdp` の観測と操作を確かめる診断ツール。
//!
//! セレクタは実機の DOM に依存しているので、リーダーの UI が変わると壊れる。
//! 壊れたときに**どこが壊れたのかを 1 コマンドで切り分ける**ために置いてある。
//!
//! 事前に、専用プロファイルでリモートデバッグを有効にした Chrome を起動し、
//! 本を開いておくこと（手順は `spikes/README.md`）。
//!
//! ```text
//! cargo run -p ke-cdp --example probe                # いま観測できることを表示
//! cargo run -p ke-cdp --example probe -- grab        # ページ画像を 1 枚取り出す
//! cargo run -p ke-cdp --example probe -- settings 9  # 白テーマ + フォント段 9 にする
//! cargo run -p ke-cdp --example probe -- turn 10     # 10 ページ送って所要時間を測る
//! cargo run -p ke-cdp --example probe -- capture <ASIN> <枚数> <記録の保存先>
//! ```
//!
//! `capture` は `ke-nav` と噛み合わせて本番と同じループを回し、
//! 観測と行動を JSON Lines に記録する（ADR-0001 §6b）。
//! **ページ画像は保存しない**（書籍の中身なので）。記録は ASIN を伏せて保存する。

use std::error::Error;
use std::thread::sleep;
use std::time::{Duration, Instant};

use ke_cdp::{Browser, CdpBrowser, Direction, Effect, Session, apply};
use ke_core::{Action, Asin, BookSpec, Observation, PageMetrics, Theme};
use ke_nav::{Limits, Navigator};

/// ページ送りの確定を待つ上限。`ke-nav` の `page_turn_timeout_ms` と同じ値にする。
const TURN_TIMEOUT: Duration = Duration::from_secs(8);
/// 表示設定を変えたあと、ページが作り直されるのを待つ上限。
const RENDER_TIMEOUT: Duration = Duration::from_secs(20);
/// 観測の間隔。
const POLL: Duration = Duration::from_millis(20);
/// 反映されないまま押し直すまでの時間（ADR-0007 実測 9。ke-nav と同じ値）。
const TURN_RETRY: Duration = Duration::from_millis(400);
/// 送りが受け付けられる最小間隔（ADR-0007 実測 9 で約 100ms）。余裕を見る。
const MIN_TURN_GAP: Duration = Duration::from_millis(120);

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut browser = CdpBrowser::connect()?;

    match args.first().map(String::as_str) {
        Some("grab") => grab(&mut browser)?,
        Some("settings") => settings(&mut browser, arg_number(&args, 1).unwrap_or(9))?,
        Some("turn") => turn(&mut browser, arg_number(&args, 1).unwrap_or(10))?,
        Some("capture") => capture_book(&mut browser, &args)?,
        Some("observe") | None => show(&browser.observe()?),
        Some(other) => {
            println!("不明な指示: {other}（observe / grab / settings / turn / capture）");
        }
    }
    Ok(())
}

fn arg_number(args: &[String], at: usize) -> Option<u32> {
    args.get(at)?.parse().ok()
}

fn show(obs: &Observation) {
    println!("位置    : {}", obs.page.as_ref().map_or("(なし)".to_owned(), ToString::to_string));
    match &obs.image {
        Some(i) => println!(
            "画像    : {}x{} complete={} src=…{}",
            i.natural_width,
            i.natural_height,
            i.complete,
            i.source.as_deref().unwrap_or("").chars().rev().take(12).collect::<String>()
        ),
        None => println!("画像    : (なし)"),
    }
    // 巻末では「次」が、先頭では「前」が消える（ADR-0007 実測 11）。
    println!(
        "送り    : {}{}{}",
        match obs.turn_controls {
            Some(c) => format!("次={} 前={}", c.next, c.prev),
            None => "(観測できず)".to_owned(),
        },
        if obs.at_start_of_book() { "  ← 先頭" } else { "" },
        if obs.at_end_of_book() { "  ← 巻末" } else { "" },
    );
    println!("メニュー: {}", if obs.settings_menu_open { "開いている" } else { "閉じている" });
    println!("フォント: {:?}", obs.font);
    println!("テーマ  : {:?}", obs.theme);
}

fn grab(browser: &mut CdpBrowser) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let image = browser.capture_page()?;
    println!(
        "{}x{} / {} bytes / {:?} / src=…{}",
        image.width,
        image.height,
        image.png.len(),
        started.elapsed(),
        image.source.as_deref().unwrap_or("").chars().rev().take(12).collect::<String>()
    );
    // PNG の署名が付いているかだけ確かめる（中身は書籍なので保存しない）。
    println!("PNG 署名: {}", image.png.starts_with(b"\x89PNG\r\n\x1a\n"));
    Ok(())
}

fn settings(browser: &mut CdpBrowser, index: u32) -> Result<(), Box<dyn Error>> {
    browser.set_settings_menu(true)?;
    sleep(Duration::from_millis(1_200));
    show(&browser.observe()?);

    browser.set_theme(Theme::White)?;
    sleep(Duration::from_millis(1_500));

    let step = u8::try_from(index).unwrap_or(u8::MAX);
    let started = Instant::now();
    browser.set_font_size(step)?;
    println!("\nフォント段 {step} にするのに {:?}", started.elapsed());

    show(&browser.observe()?);
    browser.set_settings_menu(false)?;

    // 表示設定を変えるとページが作り直され、<img> が一度 DOM から消える。
    // 戻る前に測ろうとすると「画像が無い」で落ちるので、待ち時間を知っておく。
    let waited = wait_for_render(browser)?;
    println!("\n設定後（再描画まで {waited:?}）:");
    show(&browser.observe()?);
    Ok(())
}

fn wait_for_render(browser: &mut CdpBrowser) -> Result<Duration, Box<dyn Error>> {
    let started = Instant::now();
    while started.elapsed() < RENDER_TIMEOUT {
        if browser.observe()?.has_usable_image() {
            return Ok(started.elapsed());
        }
        sleep(Duration::from_millis(20));
    }
    Err("表示設定を変えたあとページが再描画されませんでした".into())
}

fn turn(browser: &mut CdpBrowser, pages: u32) -> Result<(), Box<dyn Error>> {
    let mut times = Vec::new();
    let mut before = browser.observe()?;
    let mut turned_at = Instant::now();
    for i in 1..=pages {
        let started = Instant::now();
        let after = turn_once(browser, &before, turned_at)?;
        turned_at = Instant::now();
        let image = browser.capture_page()?;
        times.push(started.elapsed());
        println!(
            "{i:>3}: {} → {}  {:?}  {} bytes",
            before.page.as_ref().map_or("(なし)".to_owned(), ToString::to_string),
            after.page.as_ref().map_or("(なし)".to_owned(), ToString::to_string),
            started.elapsed(),
            image.png.len()
        );
        before = after;
    }
    report(&times);
    Ok(())
}

/// 1 ページ送り、確定シグナルが来るまで待つ。空振りしたら押し直す（ADR-0007 決定 4）。
///
/// `since_turn` は前回送りが通った時刻。間隔が足りないと無視されるので、
/// 足りない分だけ待ってから押す。
fn turn_once(
    browser: &mut CdpBrowser,
    before: &Observation,
    since_turn: Instant,
) -> Result<Observation, Box<dyn Error>> {
    if let Some(rest) = MIN_TURN_GAP.checked_sub(since_turn.elapsed()) {
        sleep(rest);
    }
    let deadline = Instant::now() + TURN_TIMEOUT;
    while Instant::now() < deadline {
        browser.turn_page(Direction::Next)?;
        let retry_at = Instant::now() + TURN_RETRY;
        while Instant::now() < retry_at {
            let now = browser.observe()?;
            if now.advanced_from(before) && now.has_usable_image() {
                return Ok(now);
            }
            // 間を置かずに観測し続けると、評価がページのメインスレッドを占有して
            // かえって遅くなる。
            sleep(POLL);
        }
    }
    Err("ページが進みませんでした".into())
}

/// フォント段から画素/文字を返す**仮の**実測。
///
/// 本来は OCR（`ke_ocr`）が測る値である。段と画素/文字の対応はまだ実測していない
/// （ROADMAP「残っている不確実性 #2」）ので、ADR-0005 の割合の測定を
/// 段番号に読み替えた表で代用している。**この数字を根拠にしてはいけない。**
fn stub_measure(font_index: u8) -> PageMetrics {
    let px_per_char = match font_index {
        0..=6 => 27,
        7..=10 => 45,
        _ => 51,
    };
    PageMetrics { px_per_char, chars: 0 }
}

/// 本番と同じループを回し、観測と行動を記録する。
fn capture_book(browser: &mut CdpBrowser, args: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(asin) = args.get(1) else {
        return Err("使い方: probe capture <ASIN> <枚数> <記録の保存先>".into());
    };
    let pages = arg_number(args, 2).unwrap_or(5);
    let out = args.get(3).map_or("session.jsonl", String::as_str);

    let spec = BookSpec::new(Asin::new(asin.clone())?, "実機の記録");
    // **打ち切り条件は既定のまま**にする。記録は同じ条件でしか再生できないので、
    // ここを変えるとフィクスチャとして使えなくなる。枚数は外側で押さえる。
    let mut nav = Navigator::with_limits(spec, Limits::default());

    let mut record = Session::new();
    let mut pending: Option<PageMetrics> = None;
    let mut captured = 0_u32;
    let mut bytes = 0_usize;
    let started = Instant::now();

    let last = loop {
        let mut obs = browser.observe()?;
        obs.metrics = pending.take();
        let action = nav.step(&obs);
        record.push(obs, action.clone());
        report_action(&action, record.len(), started);

        match apply(browser, &action)? {
            Effect::Terminal => break action,
            Effect::ToMeasure(_) => pending = Some(stub_measure(font_index(browser)?)),
            Effect::Captured { image, .. } => {
                captured += 1;
                bytes += image.png.len(); // 画像そのものは捨てる（書籍の中身なので）
                if captured >= pages {
                    break action;
                }
            }
            Effect::Nothing => {}
        }
    };

    println!(
        "
終了: {last:?}"
    );
    println!("撮影 {captured} 枚 / {:.1}MB / {:?}", bytes as f64 / 1e6, started.elapsed());
    let path = std::path::Path::new(out);
    record.redacted().save(path)?;
    println!("記録: {} （{} 手、ASIN は伏せた）", path.display(), record.len());
    Ok(())
}

/// 校正のために現在のフォント段を読む。設定メニューが閉じていれば既定段とみなす。
fn font_index(browser: &mut CdpBrowser) -> Result<u8, Box<dyn Error>> {
    Ok(browser.observe()?.font.map_or(5, |f| f.index))
}

/// 長い行動列を垂れ流さず、要点だけ出す。
fn report_action(action: &Action, step: usize, started: Instant) {
    match action {
        Action::Wait { .. } => {}
        Action::CapturePage { label } => {
            if step % 20 == 0 {
                println!("{step:>4} 手 {:?} {label}", started.elapsed());
            }
        }
        other => println!("{step:>4} 手 {:?} {other:?}", started.elapsed()),
    }
}

fn report(times: &[Duration]) {
    if times.is_empty() {
        return;
    }
    let mut sorted: Vec<Duration> = times.to_vec();
    sorted.sort_unstable();
    let total: Duration = times.iter().sum();
    println!(
        "\n{} 頁: 中央値 {:?} / 平均 {:?} / 最大 {:?}",
        times.len(),
        sorted[sorted.len() / 2],
        total / u32::try_from(times.len()).unwrap_or(1),
        sorted.last().copied().unwrap_or_default()
    );
}
