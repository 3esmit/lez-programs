//! Independent effects on one source holding. Never forward arbitrary internal messages.
use lee_core::program::{ReceiveInput, Response};
use token_core::{Internal, Payment};

use crate::{holding_state, token_actor, write_holding};

/// Bounds the example's message fan-out. The enclosing runtime also meters execution.
const MAX_PAYMENTS: usize = 16;

pub(crate) fn batch(input: &ReceiveInput, payments: Vec<Payment>, burn: u128) -> Response {
    assert!(input.is_authorized, "holding must authorize the batch");
    assert!(payments.len() <= MAX_PAYMENTS, "too many batch payments");
    let total = payments
        .iter()
        .try_fold(burn, |sum, payment| sum.checked_add(payment.amount))
        .expect("batch amount overflow");
    let mut holding = holding_state(input);
    holding.balance = holding
        .balance
        .checked_sub(total)
        .expect("insufficient balance");
    let definition = holding.definition_id;
    effects(
        input,
        write_holding(input, holding),
        definition,
        payments,
        burn,
    )
}

pub(crate) fn effects(
    input: &ReceiveInput,
    mut response: Response,
    definition: lee_core::account::AccountId,
    payments: Vec<Payment>,
    burn: u128,
) -> Response {
    // All sync children precede promoted async children. These effects are independent:
    // the batch API does not promise arbitrary instruction-list ordering.
    if burn != 0 {
        response = response.send_sync(
            token_actor(input, definition),
            &Internal::DecreaseSupply { amount: burn },
        );
    }
    for Payment { to, amount } in payments {
        if amount != 0 {
            response = response.send_async(
                token_actor(input, to),
                &Internal::IncreaseBalance { definition, amount },
            );
        }
    }
    response
}
