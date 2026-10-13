//! theFlash 后端。

pub mod damai;
pub mod presentation;
pub mod report;
pub mod watch;

/// 后端版本，取自 Cargo.toml。`tests/version.rs` 检查 `theflash --version`
/// 和 Cargo.toml 一致。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
