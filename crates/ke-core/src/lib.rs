//! kindle-exporter のドメイン型。
//!
//! この crate は **I/O を一切持たず、他の `ke-*` crate にも依存しない**。
//! ワークフロー全体で共有される値だけを定義する。
//!
//! この制約が `ke-nav` の状態機械を実機ゼロでテスト可能にする土台になる
//! （`docs/adr/0001-architecture.md` §6a）。
//!
//! # 構成
//!
//! - [`Asin`] / [`BookSpec`] — 何を処理するか
//! - [`Phase`] / [`Capability`] — どの工程を、どのホストで実行するか
//! - [`DisplayTarget`] / [`Theme`] / [`FontControl`] — キャプチャ時のリーダー表示設定
//! - [`PageLabel`] / [`PageImageInfo`] / [`PageMetrics`] — ページの位置・画像・品質
//! - [`Observation`] / [`Action`] — 状態機械の入力と出力

#![forbid(unsafe_code)]

mod action;
mod book;
mod display;
mod observation;
mod page;
mod phase;

pub use action::{Action, Failure, Summary, WaitReason};
pub use book::{Asin, AsinError, BookSpec};
pub use display::{DisplayTarget, FontControl, Theme};
pub use observation::Observation;
pub use page::{LabelKind, PageImageInfo, PageLabel, PageMetrics};
pub use phase::{ALL_PHASES, Capability, Phase};
