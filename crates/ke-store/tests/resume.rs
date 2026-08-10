//! 途中で落ちても再開できることを確かめる（ADR-0001 §6c）。
//!
//! この層の値打ちは「電源を切っても続きから走れる」ことなので、
//! **落ちた状態を作ってから開き直す**形のテストにしてある。

#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use ke_core::{Asin, BookSpec, PageLabel, Phase, Summary};
use ke_store::{Event, Library};

fn asin() -> Asin {
    Asin::new("B0TESTBOOK").expect("固定の ASIN")
}

fn spec() -> BookSpec {
    BookSpec::new(asin(), "テスト本")
}

/// 一時ディレクトリに蔵書を 1 冊作る。
fn library() -> (tempfile::TempDir, Library) {
    let tmp = tempfile::tempdir().expect("一時ディレクトリを作れる");
    let library = Library::open(tmp.path().join("library"));
    (tmp, library)
}

#[test]
fn registers_a_book_and_reads_it_back() {
    let (_tmp, library) = library();
    let book = library.book(&asin());
    assert!(!book.exists());

    book.register(&spec()).unwrap();

    assert!(book.exists());
    assert_eq!(book.manifest().unwrap(), spec());
    assert_eq!(library.books().unwrap(), vec![asin()]);
    // 登録そのものもイベントに残る
    assert_eq!(book.events().unwrap().len(), 1);
}

/// 登録していないディレクトリを蔵書として数えない。
#[test]
fn only_registered_books_are_listed() {
    let (_tmp, library) = library();
    std::fs::create_dir_all(library.root().join("B0NOTABOOK")).unwrap();
    assert!(library.books().unwrap().is_empty());
}

#[test]
fn saves_pages_and_tracks_where_to_continue() {
    let (_tmp, library) = library();
    let book = library.book(&asin());
    book.register(&spec()).unwrap();

    for i in 1..=3 {
        let label = PageLabel::at_location(i * 48, Some(10_167));
        book.save_page(i, b"\x89PNG\r\n\x1a\nfake", Some(label)).unwrap();
    }

    let progress = book.progress().unwrap();
    assert_eq!(progress.captured_pages, 3);
    assert_eq!(progress.next_page_index(), 4);
    assert!(book.page_path(1).exists());
    assert_eq!(book.page_path(1).file_name().unwrap(), "0001.png");
}

/// 2 枚まで撮ったところで落ちた状態を作る。ロックは解放されないまま残る。
fn crashed_after_two_pages(library: &Library) {
    let book = library.book(&asin());
    book.register(&spec()).unwrap();
    let lease = book.lease(Phase::Capture).unwrap();
    book.append(Event::PhaseStarted { phase: Phase::Capture }).unwrap();
    book.save_page(1, b"one", None).unwrap();
    book.save_page(2, b"two", None).unwrap();
    std::mem::forget(lease); // 落ちたのでロックは残る
}

/// **電源が落ちた状態から開き直す。** 撮れた分は残っている。
#[test]
fn picks_up_where_it_left_off_after_a_crash() {
    let (_tmp, library) = library();
    crashed_after_two_pages(&library);

    // まっさらな Library として開き直す
    let reopened = Library::open(library.root());
    let book = reopened.book(&asin());

    let progress = book.progress().unwrap();
    assert_eq!(progress.captured_pages, 2, "撮れた分は残っている");
    assert_eq!(progress.next_page_index(), 3, "3 枚目から続けられる");
    assert!(!progress.has_finished(Phase::Capture), "終わってはいない");
    assert_eq!(book.manifest().unwrap(), spec());
}

/// 落ちたホストのロックは残る。**勝手に奪わず、壊すのは明示的に。**
#[test]
fn a_lease_left_by_a_crash_must_be_broken_on_purpose() {
    let (_tmp, library) = library();
    crashed_after_two_pages(&library);
    let book = Library::open(library.root()).book(&asin());

    let holder = book.lease_holder(Phase::Capture).unwrap().expect("誰かが掴んでいる");
    assert_eq!(holder.pid, std::process::id());
    assert!(book.lease(Phase::Capture).is_err(), "残ったロックを黙って奪わない");

    book.break_lease(Phase::Capture).unwrap();
    let lease = book.lease(Phase::Capture).unwrap();
    assert_eq!(lease.phase(), Phase::Capture);

    book.save_page(3, b"three", None).unwrap();
    book.append(Event::PhaseFinished {
        phase: Phase::Capture,
        summary: Some(Summary { captured_pages: 3, px_per_char: Some(45), end_confirmed: true }),
    })
    .unwrap();

    let progress = book.progress().unwrap();
    assert_eq!(progress.captured_pages, 3);
    assert!(progress.has_finished(Phase::Capture));
    assert_eq!(progress.capture_summary.map(|s| s.end_confirmed), Some(true));
}

/// 再登録しても、撮れたページとイベントは失われない。
#[test]
fn registering_again_keeps_the_progress() {
    let (_tmp, library) = library();
    let book = library.book(&asin());
    book.register(&spec()).unwrap();
    book.save_page(1, b"one", None).unwrap();

    let mut changed = spec();
    changed.display = ke_core::DisplayTarget::ruby_first();
    book.register(&changed).unwrap();

    assert_eq!(book.manifest().unwrap().display, ke_core::DisplayTarget::ruby_first());
    assert_eq!(book.progress().unwrap().captured_pages, 1, "進捗を消してはいけない");
}

/// **書き終える前に落ちても、中途半端な PNG が残らない。**
/// 残ると「撮れたページ」として次のフェーズに流れてしまう。
#[test]
fn a_page_is_never_left_half_written() {
    let (_tmp, library) = library();
    let book = library.book(&asin());
    book.register(&spec()).unwrap();
    book.save_page(1, b"whole", None).unwrap();

    let leftovers: Vec<_> = std::fs::read_dir(book.page_path(1).parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty(), "書きかけが残っている: {leftovers:?}");
    assert_eq!(std::fs::read(book.page_path(1)).unwrap(), b"whole");
}

/// イベントログは追記専用である。**書き換えると履歴が失われる。**
#[test]
fn the_event_log_is_only_ever_appended_to() {
    let (_tmp, library) = library();
    let book = library.book(&asin());
    book.register(&spec()).unwrap();

    book.append(Event::PhaseStarted { phase: Phase::Capture }).unwrap();
    let after_first = book.events().unwrap();
    book.append(Event::PhaseFailed {
        phase: Phase::Capture,
        reason: "リーダーが応答しない".into(),
    })
    .unwrap();
    let after_second = book.events().unwrap();

    assert_eq!(&after_second[..after_first.len()], &after_first[..], "前の行が消えている");
    assert_eq!(after_second.len(), after_first.len() + 1);
}

/// 掴んでいる相手が分からないと、共有ストレージでは手が打てない。
#[test]
fn a_second_holder_is_told_who_has_it() {
    let (_tmp, library) = library();
    let book = library.book(&asin());
    book.register(&spec()).unwrap();

    let first = book.lease(Phase::Capture).unwrap();
    let err = book.lease(Phase::Capture).unwrap_err().to_string();
    assert!(err.contains("capture"), "{err}");
    assert!(err.contains(&std::process::id().to_string()), "{err}");

    // 別のフェーズは同時に掴める（Mac が撮りながら Windows が OCR する）
    let other = book.lease(Phase::Ocr).unwrap();
    assert_eq!(other.phase(), Phase::Ocr);

    drop(first);
    book.lease(Phase::Capture).expect("解放されたので取れる");
}

#[test]
fn a_lease_is_released_when_it_is_dropped() {
    let (_tmp, library) = library();
    let book = library.book(&asin());
    book.register(&spec()).unwrap();

    drop(book.lease(Phase::Capture).unwrap());
    assert_eq!(book.lease_holder(Phase::Capture).unwrap(), None);

    book.lease(Phase::Validate).unwrap().release().unwrap();
    assert_eq!(book.lease_holder(Phase::Validate).unwrap(), None);
}

/// 壊れたイベントログは、どの行が壊れているかまで言う。
#[test]
fn a_corrupt_event_log_says_which_line_is_bad() {
    let (_tmp, library) = library();
    let book = library.book(&asin());
    book.register(&spec()).unwrap();
    std::fs::write(book.dir().join("events.jsonl"), "こわれている\n").unwrap();

    let err = book.events().unwrap_err().to_string();
    assert!(err.contains("1 行目"), "{err}");
}
