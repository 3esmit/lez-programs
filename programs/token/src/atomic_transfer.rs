//! An ordered self-call contains a grant-free async credit. A final sibling requires the
//! recipient's acknowledgement, so deferred delivery cannot commit the debit or publication.
//! The acknowledgement follows the complete external hook subtree, not just the credit.

use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response},
};
use token_core::{Internal, Message, TokenState, TokensReceived};

use crate::{holding_state, token_actor, transfer};

pub(crate) fn start(input: &ReceiveInput, message: Message) -> Response {
    let Message::TransferAndCallAtomic {
        to,
        amount,
        callback_account,
        callback_program,
        data,
    } = message
    else {
        unreachable!("atomic transfer command")
    };
    assert!(
        input.is_authorized,
        "sender holding must authorize the transfer"
    );
    // Token actors trust internal Token messages. A user-selected hook must not enter
    // that namespace with a payload whose fields the user controls.
    assert_ne!(
        callback_program, input.receiver.program_account_id,
        "callback must use another program"
    );
    let mut holding = holding_state(input);
    holding.balance = holding
        .balance
        .checked_sub(amount)
        .expect("insufficient balance");
    Response::set_state(
        TokenState::AtomicTransfer {
            holding,
            destination: to,
            acknowledged: false,
        }
        .encode(),
    )
    .send_sync(
        input.receiver,
        &Internal::DispatchAtomicTransfer {
            to,
            amount,
            callback_account,
            callback_program,
            data,
        },
    )
    .send_sync(input.receiver, &Internal::FinishAtomicTransfer)
}

pub(crate) fn dispatch(input: &ReceiveInput, message: Internal) -> Response {
    assert_eq!(
        input.from,
        Some(input.receiver),
        "atomic dispatch must be self-sent"
    );
    let Some(TokenState::AtomicTransfer { holding, .. }) = TokenState::decode(&input.pre_state)
    else {
        panic!("no atomic transfer in progress")
    };
    let Internal::DispatchAtomicTransfer {
        to,
        amount,
        callback_account,
        callback_program,
        data,
    } = message
    else {
        unreachable!("atomic dispatch")
    };
    // An async delivery removes inherited PDA grants. The final sibling will reject it
    // unless it is promoted and the credit, hook and acknowledgement all run now.
    Response::keep_state().send_async(
        token_actor(input, to),
        &Internal::CreditAndCall {
            destination: to,
            definition: holding.definition_id,
            amount,
            callback_account,
            callback_program,
            data,
        },
    )
}

pub(crate) fn credit(input: &ReceiveInput, from: Actor, message: Internal) -> Response {
    let Internal::CreditAndCall {
        destination,
        definition,
        amount,
        callback_account,
        callback_program,
        data,
    } = message
    else {
        unreachable!("atomic credit")
    };
    transfer::increase_balance(input, from, definition, amount)
        .send_sync(
            Actor::new(callback_account, callback_program),
            &TokensReceived {
                source: from.account_id,
                destination,
                definition,
                amount,
                data,
            },
        )
        .send_sync(from, &Internal::AcknowledgeAtomicTransfer)
}

pub(crate) fn acknowledge(input: &ReceiveInput) -> Response {
    let Some(TokenState::AtomicTransfer {
        holding,
        destination,
        acknowledged,
    }) = TokenState::decode(&input.pre_state)
    else {
        panic!("no atomic transfer in progress")
    };
    assert_eq!(
        input.from,
        Some(token_actor(input, destination)),
        "unexpected atomic acknowledgement sender"
    );
    assert!(!acknowledged, "atomic transfer already acknowledged");
    Response::set_state(
        TokenState::AtomicTransfer {
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
        "atomic completion must be self-sent"
    );
    let Some(TokenState::AtomicTransfer {
        holding,
        acknowledged,
        ..
    }) = TokenState::decode(&input.pre_state)
    else {
        panic!("no atomic transfer in progress")
    };
    assert!(
        acknowledged,
        "atomic credit and callback must complete immediately"
    );
    Response::set_state(TokenState::Holding(holding).encode())
}
