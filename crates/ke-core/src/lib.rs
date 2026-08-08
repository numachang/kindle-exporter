//! kindle-exporter のドメイン型。
//!
//! この crate は **I/O を一切持たず、他の crate にも依存しない**。
//! `Book` / `Page` / `Phase` / `Margins` / `Observation` / `Action` といった、
//! ワークフロー全体で共有される値だけを定義する。
//!
//! この制約が [`ke-nav`] の状態機械を実機ゼロでテスト可能にする土台になる。
//! 詳細は `docs/adr/0001-architecture.md` を参照。
//!
//! [`ke-nav`]: https://github.com/numachang/kindle-exporter

#![forbid(unsafe_code)]

// 型定義はこれから追加する。
// 追加順は docs/adr/0001-architecture.md §5 の crate 構成に従う。

#[cfg(test)]
mod tests {
    /// ワークスペース・lint 設定・CI パイプラインが機能していることの確認。
    /// 最初の実装が入った時点で削除する。
    #[test]
    fn workspace_is_wired_up() {
        assert_eq!(2 + 2, 4);
    }
}
