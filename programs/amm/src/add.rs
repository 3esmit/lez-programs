use amm_core::{
    compute_liquidity_token_pda_seed, event::Event, mul_div_floor, DepositFunding, Message, Pending,
};
use lee_core::program::{ReceiveInput, Response};
use token_core::Message as TokenMessage;

use crate::{
    reads,
    settlement::{self, token_call},
};

pub(crate) fn execute(
    input: &ReceiveInput,
    pending: &Pending,
    funding: Option<DepositFunding>,
) -> Response {
    let Message::AddLiquidity {
        user_holding_a,
        user_holding_b,
        user_holding_lp,
        min_amount_liquidity,
        max_amount_to_add_token_a: max_a,
        max_amount_to_add_token_b: max_b,
        ..
    } = pending.request
    else {
        unreachable!("add continuation")
    };
    let mut pool = reads::pool(pending);
    let (a, b, lp) = match (funding, pending.holdings.as_slice()) {
        (None, [a, b, lp, _, _]) => (
            Some(reads::holding(a, pool.definition_token_a_id).balance),
            Some(reads::holding(b, pool.definition_token_b_id).balance),
            lp,
        ),
        (Some(deposit), [other, lp, _, _]) => {
            if deposit.definition == pool.definition_token_a_id {
                (
                    None,
                    Some(reads::holding(other, pool.definition_token_b_id).balance),
                    lp,
                )
            } else {
                assert_eq!(deposit.definition, pool.definition_token_b_id);
                (
                    Some(reads::holding(other, pool.definition_token_a_id).balance),
                    None,
                    lp,
                )
            }
        }
        (_, _) => unreachable!("liquidity holdings read"),
    };
    reads::holding(lp, pool.liquidity_pool_id);
    assert!(
        max_a != 0 && max_b != 0 && min_amount_liquidity != 0,
        "liquidity amounts must be nonzero"
    );
    assert!(
        pool.reserve_a != 0 && pool.reserve_b != 0,
        "reserves must be nonzero"
    );
    let amount_a = max_a.min(mul_div_floor(pool.reserve_a, max_b, pool.reserve_b));
    let amount_b = max_b.min(mul_div_floor(pool.reserve_b, max_a, pool.reserve_a));
    assert!(amount_a != 0 && amount_b != 0, "deposit rounds to zero");
    assert!(
        a.is_none_or(|balance| balance >= amount_a) && b.is_none_or(|balance| balance >= amount_b),
        "insufficient deposit balances"
    );
    if let Some(deposit) = funding {
        let required = if deposit.definition == pool.definition_token_a_id {
            amount_a
        } else {
            amount_b
        };
        assert_eq!(
            deposit.amount, required,
            "callback deposit does not match proportional input"
        );
    }
    let liquidity = mul_div_floor(pool.liquidity_pool_supply, amount_a, pool.reserve_a).min(
        mul_div_floor(pool.liquidity_pool_supply, amount_b, pool.reserve_b),
    );
    assert!(liquidity >= min_amount_liquidity, "liquidity below minimum");
    pool.liquidity_pool_supply = pool
        .liquidity_pool_supply
        .checked_add(liquidity)
        .expect("LP supply overflow");
    pool.reserve_a = pool
        .reserve_a
        .checked_add(amount_a)
        .expect("reserve A overflow");
    pool.reserve_b = pool
        .reserve_b
        .checked_add(amount_b)
        .expect("reserve B overflow");
    let config = pending.config.as_ref().expect("config was read");
    let mut response = Response::keep_state();
    for (balance, source, vault, amount) in [
        (a, user_holding_a, pool.vault_a_id, amount_a),
        (b, user_holding_b, pool.vault_b_id, amount_b),
    ] {
        if balance.is_some() {
            response = response.send(token_call(
                config,
                source,
                &TokenMessage::Transfer { to: vault, amount },
                None,
            ));
        }
    }
    let response = response
        .send(token_call(
            config,
            pool.liquidity_pool_id,
            &TokenMessage::Mint {
                definition: pool.liquidity_pool_id,
                to: user_holding_lp,
                amount: liquidity,
            },
            Some(compute_liquidity_token_pda_seed(input.receiver.account_id)),
        ))
        .event(
            Event::LiquidityAdded {
                amount_a,
                amount_b,
                liquidity,
            }
            .to_program_event(),
        );
    settlement::pool(input, config, pool, response)
}
