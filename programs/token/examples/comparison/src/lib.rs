//! Two comparable token-composition examples.
pub mod coordinator;
pub mod ledger;
pub mod payment;
pub mod redemption;
include!(concat!(env!("OUT_DIR"), "/methods.rs"));
