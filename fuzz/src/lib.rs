//! Library surface for the post-release fuzz helpers.
//!
//! The cargo-fuzz bins include `oracle.rs` directly. This crate exists so
//! `cargo test --manifest-path fuzz/Cargo.toml --lib` can check the quarantine
//! predicates that kept the half-hour campaign running.

pub mod oracle;

#[cfg(test)]
mod acceptance {
    use super::oracle;

    #[test]
    fn numbered_parameter_over_the_sqlite_cap_is_quarantined() {
        assert!(!oracle::numbered_parameter_exceeds_sqlite_cap("SELECT ?1"));
        assert!(!oracle::numbered_parameter_exceeds_sqlite_cap("SELECT ?32766"));
        assert!(oracle::numbered_parameter_exceeds_sqlite_cap("SELECT ?32767"));
        assert!(oracle::numbered_parameter_exceeds_sqlite_cap("SELECT ?123456"));
        assert!(!oracle::skips_recursive("SELECT 1"));
        assert!(oracle::skips_recursive(
            "WITH RECURSIVE c(x) AS (SELECT 1) SELECT x FROM c",
        ));
    }
}
