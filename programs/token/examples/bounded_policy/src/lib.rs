//! Two comparable token-composition examples.
pub mod payment;
pub mod policy;
pub mod redemption;
include!(concat!(env!("OUT_DIR"), "/methods.rs"));
