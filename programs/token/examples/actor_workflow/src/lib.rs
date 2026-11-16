//! The payment example using authenticated reads, Pending, Settling and self-completion.
//! Runs with the existing Token Program; no transfer hook or Token state extension is needed.
//!
//! Run the complete example with:
//! ```text
//! RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_workflow
//! ```
//! With an initial payer balance of 1,000, processing 100 pays 60, burns 10 and refunds
//! 30. The final payer balance is 930, the vault balance is zero, and supply is 990.
//! The same business result is exercised by the separate atomic-callback prototype.
//!
//! `Pending` retains the request while source, vault and beneficiary replies arrive.
//! Each reply must come from the exact account and configured Token deployment.
//! `Settling` protects the prepared result while Token transfers and the burn execute.
//! This example also reads final balances before its self-sent `Complete` restores
//! `Ready`; a final root check prevents an unanswered read from committing `Pending`.
//! Snapshots provide information. The Token still checks authorization for each debit.
//!
//! The executable scenarios use public participants. Private and blinded flows need
//! separate integration coverage before this example is used as a production protocol.
pub mod receiver;
include!(concat!(env!("OUT_DIR"), "/methods.rs"));
