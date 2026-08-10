//! フェーズの実行と再開。
//!
//! ここが合流点である。**この crate 自身は何も判断しない。**
//!
//! | 何を | どこが |
//! |---|---|
//! | 次に何をすべきか | `ke-nav`（I/O を持たない状態機械） |
//! | それをブラウザにどうやらせるか | `ke-cdp` |
//! | 結果をどこに置くか | `ke-store` |
//! | 画素/文字をどう測るか | [`Measurer`]（OCR を持つ層） |
//!
//! この層がやるのは、その 4 つを回して**途中で落ちても続きから走れるようにする**
//! ことだけである。

#![forbid(unsafe_code)]

mod capture;
mod error;
mod measure;

pub use capture::{CaptureOptions, Outcome, capture};
pub use error::{Error, Result};
pub use measure::{Measurer, StubMeasurer};
