//! Build metadata baked in by `build.rs`.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("A2M_COMMIT");
/// `&'static str` rather than a function because clap's `version` attribute needs one.
pub const LONG_VERSION: &str = env!("A2M_LONG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn long_version_carries_both_parts() {
        assert!(super::LONG_VERSION.starts_with(super::VERSION));
        assert!(super::LONG_VERSION.contains(super::COMMIT));
    }
}
