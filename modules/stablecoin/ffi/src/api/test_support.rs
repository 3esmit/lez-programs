//! Helpers for tests that run program handlers directly.

use lee_core::{
    account::{Account, AccountId, AccountWithMetadata},
    program::{post_state, AccountStateDiff},
};

/// The account `diff` leaves behind when `executing_program` runs the handler.
pub(super) fn post_account(diff: &AccountStateDiff, executing_program: AccountId) -> Account {
    post_state(diff, executing_program).expect("balance diff applies to the pre-state balance")
}

/// What a chained call's callee receives for `account_id`: the caller's post-state for
/// that account. The runtime sets `is_authorized` (from a PDA seed or an original signer),
/// so the test passes the value it expects.
pub(super) fn chained_input(
    caller_diffs: &[AccountStateDiff],
    caller: AccountId,
    account_id: AccountId,
    is_authorized: bool,
) -> AccountWithMetadata {
    let diff = caller_diffs
        .iter()
        .find(|diff| diff.pre_state.account_id == account_id)
        .expect("chained call input must be one of the caller's accounts");
    AccountWithMetadata::new(post_account(diff, caller), is_authorized, account_id)
}
