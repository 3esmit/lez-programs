//! Proportional two-asset redemption with authenticated reads and settlement checks.
//! Run: `RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_redemption`.
//! See README.md for the formula, transaction boundary and example-only scope.
pub mod redemption;
include!(concat!(env!("OUT_DIR"), "/methods.rs"));
