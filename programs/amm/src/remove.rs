use amm_core::{
    compute_vault_pda_seed, event::Event, mul_div_floor, AmmConfig, CheckedRemoval, Message,
    Pending, PoolDefinition, MINIMUM_LIQUIDITY,
};
use lee_core::program::{ReceiveInput, Response};
use token_core::Message as TokenMessage;

use crate::{
    reads,
    settlement::{self, token_call},
};

pub(crate) fn execute(input: &ReceiveInput, pending: &Pending) -> Response {
    let Message::RemoveLiquidity {
        user_holding_a,
        user_holding_b,
        user_holding_lp,
        remove_liquidity_amount: liquidity,
        min_amount_to_remove_token_a: min_a,
        min_amount_to_remove_token_b: min_b,
        ..
    } = pending.request
    else {
        unreachable!("remove continuation")
    };
    let pool = reads::pool(pending);
    let [a, b, lp, _, _] = pending.holdings.as_slice() else {
        unreachable!("five holdings read")
    };
    reads::holding(a, pool.definition_token_a_id);
    reads::holding(b, pool.definition_token_b_id);
    let lp = reads::holding(lp, pool.liquidity_pool_id);
    let amounts = amounts(&pool, lp.balance, liquidity, min_a, min_b);
    let (amount_a, amount_b) = amounts;
    let config = pending.config.as_ref().expect("config was read");
    // Burn synchronously reports the supply decrease to the LP definition.
    let response = Response::keep_state()
        .send(token_call(
            config,
            user_holding_lp,
            &TokenMessage::Burn { amount: liquidity },
            None,
        ))
        .send(token_call(
            config,
            pool.vault_a_id,
            &TokenMessage::Transfer {
                to: user_holding_a,
                amount: amount_a,
            },
            Some(compute_vault_pda_seed(
                input.receiver.account_id,
                pool.definition_token_a_id,
            )),
        ))
        .send(token_call(
            config,
            pool.vault_b_id,
            &TokenMessage::Transfer {
                to: user_holding_b,
                amount: amount_b,
            },
            Some(compute_vault_pda_seed(
                input.receiver.account_id,
                pool.definition_token_b_id,
            )),
        ));
    finish(input, config, pool, response, liquidity, amounts)
}

pub(crate) fn execute_checked(
    input: &ReceiveInput,
    mut pool: PoolDefinition,
    config: &AmmConfig,
    request: &CheckedRemoval,
) -> Response {
    let quote = &request.quote;
    assert_eq!(
        (pool.reserve_a, pool.reserve_b),
        (quote.reserve_a, quote.reserve_b),
        "stale recorded reserves"
    );
    assert!(
        quote.vault_a_balance >= pool.reserve_a && quote.vault_b_balance >= pool.reserve_b,
        "quoted vault balances below recorded reserves"
    );
    // BurnChecked checks the definition's actual supply before that supply changes.
    pool.liquidity_pool_supply = quote.lp_supply;
    let amounts = amounts(
        &pool,
        quote.lp_balance,
        request.liquidity,
        request.min_a,
        request.min_b,
    );
    assert_eq!(
        amounts,
        (quote.amount_a, quote.amount_b),
        "incorrect quoted outputs"
    );

    let mut response = Response::keep_state().send(token_call(
        config,
        request.user_holding_lp,
        &TokenMessage::BurnChecked {
            definition: pool.liquidity_pool_id,
            expected_balance: quote.lp_balance,
            expected_supply: quote.lp_supply,
            amount: request.liquidity,
        },
        None,
    ));
    for (vault, definition, vault_balance, recipient, recipient_balance, amount) in [
        (
            pool.vault_a_id,
            pool.definition_token_a_id,
            quote.vault_a_balance,
            request.user_holding_a,
            quote.recipient_a_balance,
            amounts.0,
        ),
        (
            pool.vault_b_id,
            pool.definition_token_b_id,
            quote.vault_b_balance,
            request.user_holding_b,
            quote.recipient_b_balance,
            amounts.1,
        ),
    ] {
        // Token completes the credit and acknowledges it before the next AMM call.
        // A credit to the same holding offsets its debit, as in ordinary removal.
        let final_balance = if vault == recipient {
            recipient_balance
        } else {
            recipient_balance
                .checked_add(amount)
                .expect("quoted recipient balance overflow")
        };
        response = response
            .send(token_call(
                config,
                vault,
                &TokenMessage::TransferChecked {
                    definition,
                    expected_balance: vault_balance,
                    to: recipient,
                    amount,
                },
                Some(compute_vault_pda_seed(
                    input.receiver.account_id,
                    definition,
                )),
            ))
            .send(token_call(
                config,
                recipient,
                &TokenMessage::AssertHolding {
                    definition,
                    balance: final_balance,
                },
                None,
            ));
    }
    finish(input, config, pool, response, request.liquidity, amounts)
}

fn amounts(
    pool: &PoolDefinition,
    lp_balance: u128,
    liquidity: u128,
    min_a: u128,
    min_b: u128,
) -> (u128, u128) {
    assert!(
        liquidity != 0 && min_a != 0 && min_b != 0,
        "withdrawal amounts must be nonzero"
    );
    assert!(liquidity <= lp_balance, "insufficient LP balance");
    let unlocked = pool
        .liquidity_pool_supply
        .checked_sub(MINIMUM_LIQUIDITY)
        .expect("minimum liquidity is locked");
    assert!(
        liquidity <= unlocked,
        "cannot remove locked minimum liquidity"
    );
    let amount_a = mul_div_floor(pool.reserve_a, liquidity, pool.liquidity_pool_supply);
    let amount_b = mul_div_floor(pool.reserve_b, liquidity, pool.liquidity_pool_supply);
    assert!(
        amount_a >= min_a && amount_b >= min_b,
        "withdrawal below minimum"
    );
    (amount_a, amount_b)
}

fn finish(
    input: &ReceiveInput,
    config: &AmmConfig,
    mut pool: PoolDefinition,
    response: Response,
    liquidity: u128,
    (amount_a, amount_b): (u128, u128),
) -> Response {
    pool.liquidity_pool_supply = pool
        .liquidity_pool_supply
        .checked_sub(liquidity)
        .expect("LP supply underflow");
    pool.reserve_a = pool
        .reserve_a
        .checked_sub(amount_a)
        .expect("reserve A underflow");
    pool.reserve_b = pool
        .reserve_b
        .checked_sub(amount_b)
        .expect("reserve B underflow");
    settlement::pool(
        input,
        config,
        pool,
        response.event(
            Event::LiquidityRemoved {
                amount_a,
                amount_b,
                liquidity,
            }
            .to_program_event(),
        ),
    )
}
