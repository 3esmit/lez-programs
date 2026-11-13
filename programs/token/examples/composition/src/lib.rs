//! Payment composition with the existing Token Program and LEZ runtime.
//!
//! Run: `RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_composition`.
//! See `README.md` for the distinct custody and authorization guarantees.
pub mod processor;
include!(concat!(env!("OUT_DIR"), "/methods.rs"));
