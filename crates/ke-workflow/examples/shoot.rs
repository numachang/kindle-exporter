//! 実機のリーダーから 1 冊を撮り、保管庫に置く。
//!
//! `ke-cli` が出来るまでの繋ぎである。本番と同じ経路
//! （`ke-nav` → `ke-cdp` → `ke-store`）を通るので、これで撮ったものは
//! そのまま後段のフェーズに流せる。
//!
//! 事前に、専用プロファイルでリモートデバッグを有効にした Chrome を起動し、
//! 本を開いておくこと（起動フラグを含む手順は `docs/ROADMAP.md`）。
//!
//! ```text
//! cargo run -p ke-workflow --example shoot -- <ASIN> <保管庫のパス> [枚数の上限]
//! ```
//!
//! **画素/文字の実測はまだできない**（Python の OCR ワーカーが無い）。
//! 仮の値で走らせるので、記録される `px_per_char` は測定結果ではない。

use std::error::Error;
use std::time::Instant;

use ke_cdp::CdpBrowser;
use ke_core::{Asin, BookSpec};
use ke_nav::Limits;
use ke_store::Library;
use ke_workflow::{CaptureOptions, Outcome, StubMeasurer, capture};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(asin), Some(root)) = (args.first(), args.get(1)) else {
        return Err("使い方: shoot <ASIN> <保管庫のパス> [枚数の上限]".into());
    };
    let max_pages = args.get(2).and_then(|s| s.parse().ok());

    let library = Library::open(root);
    let book = library.book(&Asin::new(asin.clone())?);
    if !book.exists() {
        book.register(&BookSpec::new(Asin::new(asin.clone())?, "実機から取得"))?;
        println!("登録しました: {}", book.dir().display());
    }

    let options = CaptureOptions {
        limits: Limits { max_pages: max_pages.unwrap_or(5_000), ..Limits::default() },
        ..CaptureOptions::default()
    };
    let mut browser = CdpBrowser::connect()?;
    // OCR がまだ無いので、既定の目標に収まる値を返すだけの仮の実測を使う。
    let mut measurer = StubMeasurer::returning(45);

    let started = Instant::now();
    let outcome = capture(&book, &mut browser, &mut measurer, &options)?;
    report(&outcome, started);

    let progress = book.progress()?;
    println!("保管庫: {}", book.dir().display());
    println!("保存済み: {} 枚", progress.captured_pages);
    Ok(())
}

fn report(outcome: &Outcome, started: Instant) {
    let elapsed = started.elapsed();
    let pages = outcome.captured_pages();
    match outcome {
        Outcome::Finished(s) => {
            println!("\n撮り切りました: {pages} 枚 / {elapsed:?} / 巻末確定={}", s.end_confirmed)
        }
        Outcome::Failed { failure, .. } => {
            println!("\n打ち切りました: {failure:?} / {pages} 枚 / {elapsed:?}");
        }
        Outcome::AlreadyDone(_) => println!("\n既に撮り終えています（撮り直すには force）"),
    }
    if pages > 0 {
        let each = elapsed / pages;
        println!("1 枚あたり {each:?}");
    }
}
