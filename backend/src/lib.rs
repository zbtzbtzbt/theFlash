//! theFlash 后端。

pub mod damai;
pub mod presentation;
pub mod report;
pub mod watch;

/// 后端版本，取自 Cargo.toml。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_cargo_toml() {
        assert_eq!(version(), "0.1.0");
    }
}
