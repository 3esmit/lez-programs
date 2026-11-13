//! Executable example: receive a transfer, pay a beneficiary, burn tokens and refund change.
//! The extra guests exercise attempted source reentry and PDA authorization isolation.
//!
//! Run the complete example with:
//! ```text
//! RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_callback
//! ```
//! Starting with 1,000 tokens, a transfer of 100 pays 60 to the beneficiary, burns 10,
//! and refunds 30. The final payer balance is 930, the vault balance is zero, and supply
//! is 990. The tests also reject after all effects and check that actor data is restored.
//!
//! The Token holds its source in `AtomicTransfer`, permits incoming refunds, and blocks
//! public operations on that source until the recipient acknowledges its hook subtree.
//! A self-dispatch nests a grant-free async credit before the final self-completion check.
//! If that credit is deferred, the acknowledgement is absent and the operation fails.
//!
//! This is an opt-in hook, not an isolated plugin runtime. Inherited PDA grants stop at
//! the credit, but transaction credentials remain available. The example spends its own
//! vault with its own PDA grant and limits its distribution to the authenticated receipt.
//! Other authorized accounts are not sandboxed by this interface.
//!
//! The hook must authenticate both the recipient account and the Token deployment.
//! Caller-provided receipt data is an instruction, not an authorization. Hook success
//! means its immediate subtree succeeded; it does not prove acceptance of any deferred
//! messages that the hook deliberately emits.
//!
//! The runnable distribution uses public actors. A proof-path regression rejects a
//! deferred private credit; successful blinded private flows need separate coverage.
pub mod receiver;
include!(concat!(env!("OUT_DIR"), "/methods.rs"));
