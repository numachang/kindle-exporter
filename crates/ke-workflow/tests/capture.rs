//! 1 冊を撮り切り、途中で落ちても続きから走れることを、実機ゼロで確かめる。
//!
//! ここまで来ると、`ke-nav`（判断）・`ke-cdp`（実行）・`ke-store`（保管）が
//! 実際に噛み合っているかが分かる。派生元の `kindle_shot` にはこの層の
//! テストが 1 件も無かった（ADR-0001 §6）。

#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use ke_cdp::FakeBrowser;
use ke_core::{Asin, BookSpec, Failure, Phase};
use ke_store::{Event, Library};
use ke_workflow::{CaptureOptions, Outcome, StubMeasurer, capture};

fn asin() -> Asin {
    Asin::new("B0TESTBOOK").expect("固定の ASIN")
}

fn spec() -> BookSpec {
    BookSpec::new(asin(), "テスト本")
}

/// 一時ディレクトリに登録済みの書籍を 1 冊用意する。
fn shelf() -> (tempfile::TempDir, Library) {
    let tmp = tempfile::tempdir().expect("一時ディレクトリを作れる");
    let library = Library::open(tmp.path().join("library"));
    library.book(&asin()).register(&spec()).expect("登録できる");
    (tmp, library)
}

/// 既定の目標（画素/文字 40〜50）に一発で収まる仮の実測。
fn measurer() -> StubMeasurer {
    StubMeasurer::returning(45)
}

#[test]
fn captures_a_whole_book_into_the_library() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());
    let mut browser = FakeBrowser::with_pages(8).starting_at(4);

    let outcome =
        capture(&book, &mut browser, &mut measurer(), &CaptureOptions::default()).unwrap();

    assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
    assert_eq!(outcome.captured_pages(), 8);

    let progress = book.progress().unwrap();
    assert_eq!(progress.captured_pages, 8);
    assert!(progress.has_finished(Phase::Capture));

    // 画像が実際に置かれている
    for i in 1..=8 {
        assert!(book.page_path(i).exists(), "{i} 枚目が無い");
    }
    assert!(!book.page_path(9).exists());
}

/// 記録が書籍の脇に残る。事故が起きたときに持ち帰るためのもの（ADR-0001 §6b）。
#[test]
fn leaves_a_replayable_record_beside_the_book() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());

    capture(&book, &mut FakeBrowser::with_pages(4), &mut measurer(), &CaptureOptions::default())
        .unwrap();

    let dir = book.dir().join("sessions");
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
    assert_eq!(files.len(), 1, "記録が 1 本残る");

    let record = ke_cdp::Session::load(&files[0].path()).unwrap();
    assert!(record.len() > 10);
    // そのままフィクスチャにできるよう ASIN は伏せてある
    let text = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(text.contains("B0TESTBOOK"));
}

#[test]
fn does_not_record_when_it_is_told_not_to() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());
    let options = CaptureOptions { record: false, ..CaptureOptions::default() };

    capture(&book, &mut FakeBrowser::with_pages(3), &mut measurer(), &options).unwrap();

    assert!(!book.dir().join("sessions").exists());
}

/// **落ちたあとにもう一度走らせると、先頭から撮り直して撮り切る。**
/// 連番は先頭から数えた枚数なので、上書きしても矛盾しない。
#[test]
fn a_second_run_after_a_crash_completes_the_book() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());

    // 1 回目: 5 枚しか撮れない打ち切り条件で走らせ、失敗させる
    let stingy = CaptureOptions {
        limits: ke_nav::Limits { max_pages: 5, ..ke_nav::Limits::default() },
        ..CaptureOptions::default()
    };
    let first = capture(&book, &mut FakeBrowser::with_pages(9), &mut measurer(), &stingy).unwrap();
    // 上限に達した時点で打ち切るが、そこまでの 5 枚は残る
    assert_eq!(
        first,
        Outcome::Failed { failure: Failure::TooManyPages { limit: 5 }, captured_pages: 5 }
    );
    assert_eq!(book.progress().unwrap().captured_pages, 5);
    assert!(!book.progress().unwrap().has_finished(Phase::Capture));

    // 2 回目: 既定の条件で走らせ直す
    let second = capture(
        &book,
        &mut FakeBrowser::with_pages(9),
        &mut measurer(),
        &CaptureOptions::default(),
    )
    .unwrap();

    assert!(matches!(second, Outcome::Finished(_)), "{second:?}");
    assert_eq!(book.progress().unwrap().captured_pages, 9, "撮り直しを二重に数えない");
    for i in 1..=9 {
        assert!(book.page_path(i).exists(), "{i} 枚目が無い");
    }
}

/// 完了済みの書籍は、黙って撮り直さない。
#[test]
fn a_finished_book_is_left_alone() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());
    let done = capture(
        &book,
        &mut FakeBrowser::with_pages(3),
        &mut measurer(),
        &CaptureOptions::default(),
    )
    .unwrap();

    let again = capture(
        &book,
        &mut FakeBrowser::with_pages(3),
        &mut measurer(),
        &CaptureOptions::default(),
    )
    .unwrap();

    assert!(matches!(again, Outcome::AlreadyDone(_)), "{again:?}");
    assert_eq!(again.captured_pages(), done.captured_pages());

    // force を付ければ撮り直す
    let forced = CaptureOptions { force: true, ..CaptureOptions::default() };
    let redone = capture(&book, &mut FakeBrowser::with_pages(3), &mut measurer(), &forced).unwrap();
    assert!(matches!(redone, Outcome::Finished(_)), "{redone:?}");
}

/// リーダーの都合で撮り切れないのは異常ではない。**結果として記録して次へ進む。**
#[test]
fn a_reader_failure_is_recorded_rather_than_thrown() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());

    let outcome = capture(
        &book,
        &mut FakeBrowser::with_pages(5).with_broken_menu(),
        &mut measurer(),
        &CaptureOptions::default(),
    )
    .unwrap();

    assert_eq!(
        outcome,
        Outcome::Failed { failure: Failure::SettingsMenuUnavailable, captured_pages: 0 }
    );
    let failures: Vec<_> = book
        .events()
        .unwrap()
        .into_iter()
        .filter(|r| matches!(r.event, Event::PhaseFailed { .. }))
        .collect();
    assert_eq!(failures.len(), 1, "失敗もイベントに残る");
}

/// 実測（OCR）が落ちたら、撮影を続けずに止める。
#[test]
fn a_failing_measurer_stops_the_run() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());
    let mut broken = StubMeasurer::returning(45).failing_after(0);

    let err =
        capture(&book, &mut FakeBrowser::with_pages(4), &mut broken, &CaptureOptions::default())
            .unwrap_err();

    assert!(err.to_string().contains("実測"), "{err}");
    assert_eq!(book.progress().unwrap().captured_pages, 0);
}

/// 同じ書籍を 2 台で同時に撮らない。**誰が掴んでいるかまで返す。**
#[test]
fn two_hosts_cannot_capture_the_same_book_at_once() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());
    let held = book.lease(Phase::Capture).unwrap();

    let err = capture(
        &book,
        &mut FakeBrowser::with_pages(3),
        &mut measurer(),
        &CaptureOptions::default(),
    )
    .unwrap_err();

    assert!(err.to_string().contains("capture"), "{err}");
    assert!(err.to_string().contains(&std::process::id().to_string()), "{err}");
    drop(held);
}

/// 走り終わればロックは解放されている（次のフェーズが取れる）。
#[test]
fn the_lease_is_released_when_the_run_ends() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());

    capture(&book, &mut FakeBrowser::with_pages(3), &mut measurer(), &CaptureOptions::default())
        .unwrap();

    assert_eq!(book.lease_holder(Phase::Capture).unwrap(), None);
    book.lease(Phase::Capture).expect("取れる");
}

/// 位置表示を持たない書籍でも、連番で保存できる。
#[test]
fn captures_a_book_without_any_position_display() {
    let (_tmp, library) = shelf();
    let book = library.book(&asin());

    let outcome = capture(
        &book,
        &mut FakeBrowser::with_pages(5).without_labels(),
        &mut measurer(),
        &CaptureOptions::default(),
    )
    .unwrap();

    assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
    assert_eq!(book.progress().unwrap().captured_pages, 5);
}
