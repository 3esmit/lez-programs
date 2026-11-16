//! Token-level spending permission; it does not restrict ambient runtime credentials.
use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response},
};
use token_core::{Payment, SpendPermission, TokenState};

use crate::{batch, holding_state};

fn generation(input: &ReceiveInput) -> u64 {
    match TokenState::decode(&input.pre_state) {
        Some(TokenState::PermittedHolding { generation, .. }) => generation,
        Some(TokenState::Holding(_)) => 0,
        Some(TokenState::Definition(_)) | None => panic!("receiver is not a token holding"),
    }
}

pub(crate) fn approve(input: &ReceiveInput, permission: SpendPermission) -> Response {
    assert!(
        input.is_authorized,
        "owner must authorize spending approval"
    );
    assert_ne!(
        permission.spender_program, input.receiver.program_account_id,
        "spender must use another program"
    );
    assert!(
        permission.burn_remaining <= permission.remaining,
        "burn budget exceeds total budget"
    );
    assert_eq!(
        permission.next_nonce, 0,
        "new permission nonce must be zero"
    );
    let generation = generation(input)
        .checked_add(1)
        .expect("permission generation overflow");
    Response::set_state(
        TokenState::PermittedHolding {
            holding: holding_state(input),
            generation,
            permission: Some(permission),
        }
        .encode(),
    )
}

pub(crate) fn revoke(input: &ReceiveInput) -> Response {
    assert!(
        input.is_authorized,
        "owner must authorize spending revocation"
    );
    Response::set_state(
        TokenState::PermittedHolding {
            holding: holding_state(input),
            generation: generation(input),
            permission: None,
        }
        .encode(),
    )
}

pub(crate) fn spend(
    input: &ReceiveInput,
    expected_generation: u64,
    nonce: u64,
    pay: u128,
    burn: u128,
) -> Response {
    let Some(TokenState::PermittedHolding {
        mut holding,
        generation,
        permission: Some(mut permission),
    }) = TokenState::decode(&input.pre_state)
    else {
        panic!("no active spending permission")
    };
    assert_eq!(generation, expected_generation, "stale spending permission");
    assert_eq!(
        input.from,
        Some(Actor::new(
            permission.spender_account,
            permission.spender_program
        )),
        "caller is not the permitted actor"
    );
    assert_eq!(permission.next_nonce, nonce, "stale spending nonce");
    permission.next_nonce = nonce.checked_add(1).expect("spending nonce overflow");
    let total = pay.checked_add(burn).expect("spending amount overflow");
    assert_ne!(total, 0, "empty delegated spend");
    permission.remaining = permission
        .remaining
        .checked_sub(total)
        .expect("spending permission exhausted");
    permission.burn_remaining = permission
        .burn_remaining
        .checked_sub(burn)
        .expect("burn permission exhausted");
    holding.balance = holding
        .balance
        .checked_sub(total)
        .expect("insufficient balance");
    let definition = holding.definition_id;
    let destination = permission.destination;
    let expires_before = permission.expires_before;
    // Consume both limits before producing any effects. A failure rolls consumption back
    // with the rest of this application operation. No credential is borrowed from the owner.
    let response = Response::set_state(
        TokenState::PermittedHolding {
            holding,
            generation,
            permission: Some(permission),
        }
        .encode(),
    )
    .timestamp_window(..expires_before);
    batch::effects(
        input,
        response,
        definition,
        vec![Payment {
            to: destination,
            amount: pay,
        }],
        burn,
    )
}
