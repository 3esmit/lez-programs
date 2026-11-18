//! Local compare/use operations. Required credits are acknowledged before completion.
//! Conditions across different actors do not constitute a global snapshot.
use lee_core::{
    account::{AccountId, Actor},
    program::{ReceiveInput, Response},
};
use token_core::{Internal, TokenState};

use crate::{burn, definition_state, holding_state, token_actor, transfer};

pub(crate) fn assert_holding(input: &ReceiveInput, definition: AccountId, balance: u128) {
    let holding = holding_state(input);
    assert_eq!(
        holding.definition_id, definition,
        "unexpected token definition"
    );
    assert_eq!(holding.balance, balance, "stale holding condition");
}

pub(crate) fn assert_supply(input: &ReceiveInput, supply: u128) {
    assert_eq!(
        definition_state(input).total_supply,
        supply,
        "stale supply condition"
    );
}

pub(crate) fn transfer_checked(
    input: &ReceiveInput,
    definition: AccountId,
    expected_balance: u128,
    to: AccountId,
    amount: u128,
) -> Response {
    assert!(
        input.is_authorized,
        "sender holding must authorize the transfer"
    );
    assert_holding(input, definition, expected_balance);
    let mut holding = holding_state(input);
    holding.balance = holding
        .balance
        .checked_sub(amount)
        .expect("insufficient balance");
    Response::set_state(
        TokenState::CheckedTransfer {
            holding,
            destination: to,
            acknowledged: false,
        }
        .encode(),
    )
    .send_sync(
        input.receiver,
        &Internal::DispatchCheckedCredit { to, amount },
    )
    .send_sync(input.receiver, &Internal::FinishCheckedTransfer)
}

pub(crate) fn burn_checked(
    input: &ReceiveInput,
    definition: AccountId,
    expected_balance: u128,
    expected_supply: u128,
    amount: u128,
) -> Response {
    assert!(input.is_authorized, "holding must authorize the burn");
    assert_holding(input, definition, expected_balance);
    let mut holding = holding_state(input);
    holding.balance = holding
        .balance
        .checked_sub(amount)
        .expect("insufficient balance");
    Response::set_state(TokenState::Holding(holding).encode()).send_sync(
        token_actor(input, definition),
        &Internal::DecreaseSupplyChecked {
            expected_supply,
            amount,
        },
    )
}

pub(crate) fn decrease_supply(
    input: &ReceiveInput,
    from: Actor,
    expected_supply: u128,
    amount: u128,
) -> Response {
    assert_supply(input, expected_supply);
    burn::decrease_supply(input, from, amount)
}

pub(crate) fn dispatch(input: &ReceiveInput, to: AccountId, amount: u128) -> Response {
    assert_eq!(
        input.from,
        Some(input.receiver),
        "checked dispatch must be self-sent"
    );
    let Some(TokenState::CheckedTransfer {
        holding,
        destination,
        ..
    }) = TokenState::decode(&input.pre_state)
    else {
        panic!("no checked transfer")
    };
    assert_eq!(to, destination, "checked destination mismatch");
    // Promotion removes inherited PDA grants. The final sibling rejects deferred credit.
    Response::keep_state().send_async(
        token_actor(input, to),
        &Internal::CheckedCredit {
            definition: holding.definition_id,
            amount,
        },
    )
}

pub(crate) fn credit(
    input: &ReceiveInput,
    from: Actor,
    definition: AccountId,
    amount: u128,
) -> Response {
    transfer::increase_balance(input, from, definition, amount)
        .send_sync(from, &Internal::AcknowledgeCheckedCredit)
}

pub(crate) fn acknowledge(input: &ReceiveInput) -> Response {
    let Some(TokenState::CheckedTransfer {
        holding,
        destination,
        acknowledged,
    }) = TokenState::decode(&input.pre_state)
    else {
        panic!("no checked transfer")
    };
    assert_eq!(
        input.from,
        Some(token_actor(input, destination)),
        "unexpected checked acknowledgement"
    );
    assert!(!acknowledged, "checked credit already acknowledged");
    Response::set_state(
        TokenState::CheckedTransfer {
            holding,
            destination,
            acknowledged: true,
        }
        .encode(),
    )
}

pub(crate) fn finish(input: &ReceiveInput) -> Response {
    assert_eq!(
        input.from,
        Some(input.receiver),
        "checked finish must be self-sent"
    );
    let Some(TokenState::CheckedTransfer {
        holding,
        acknowledged,
        ..
    }) = TokenState::decode(&input.pre_state)
    else {
        panic!("no checked transfer")
    };
    assert!(acknowledged, "checked credit must complete immediately");
    Response::set_state(TokenState::Holding(holding).encode())
}
