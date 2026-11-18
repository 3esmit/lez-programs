//! Settlement conditions derived from a bounded set of accounts per operation.
//!
//! Quoted values are usable for calculation only because the same message tree
//! checks each value against its owning Token actor before completion.

use std::collections::{btree_map::Entry, BTreeMap};

use amm_core::{
    AmmConfig, Message as AmmMessage, Pending, PoolDefinition, PoolQuote, MINIMUM_LIQUIDITY,
};
use lee_core::{
    account::AccountId,
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use token_core::{Message, TokenHolding};

use crate::{add, new_definition, reads, remove, settlement::token_call, swap, sync};

#[derive(Default)]
pub(crate) struct Balances(BTreeMap<AccountId, Balance>);

struct Balance {
    definition: AccountId,
    amount: u128,
    // A checked debit verifies this entry. Otherwise verify it after settlement.
    needs_assertion: bool,
}

impl Balances {
    pub(crate) fn remember(
        &mut self,
        account: AccountId,
        definition: AccountId,
        amount: u128,
    ) -> Option<TokenHolding> {
        match self.0.entry(account) {
            Entry::Vacant(entry) => {
                entry.insert(Balance {
                    definition,
                    amount,
                    needs_assertion: true,
                });
            }
            Entry::Occupied(entry) => {
                assert_eq!(
                    (entry.get().definition, entry.get().amount),
                    (definition, amount),
                    "inconsistent quoted holding"
                );
            }
        }
        Some(TokenHolding {
            definition_id: definition,
            balance: amount,
        })
    }

    pub(crate) fn vaults(
        &mut self,
        pool: &PoolDefinition,
        quote: &PoolQuote,
    ) -> [Option<TokenHolding>; 2] {
        assert_eq!(
            (pool.reserve_a, pool.reserve_b),
            (quote.reserve_a, quote.reserve_b),
            "stale recorded reserves"
        );
        assert!(
            quote.vault_a_balance >= pool.reserve_a && quote.vault_b_balance >= pool.reserve_b,
            "quoted vault balances below recorded reserves"
        );
        [
            self.remember(
                pool.vault_a_id,
                pool.definition_token_a_id,
                quote.vault_a_balance,
            ),
            self.remember(
                pool.vault_b_id,
                pool.definition_token_b_id,
                quote.vault_b_balance,
            ),
        ]
    }

    pub(crate) fn add_credit(&mut self, account: AccountId, definition: AccountId, amount: u128) {
        if let Some(entry) = self.0.get_mut(&account) {
            assert_eq!(entry.definition, definition, "incompatible quote alias");
            entry.amount = entry
                .amount
                .checked_add(amount)
                .expect("quoted credit overflow");
        }
    }

    pub(crate) fn transfer(
        &mut self,
        config: &AmmConfig,
        from: AccountId,
        to: AccountId,
        definition: AccountId,
        amount: u128,
        seed: Option<PdaSeed>,
    ) -> SyncMessage {
        let source = self.0.get_mut(&from).expect("checked sender balance");
        assert_eq!(source.definition, definition, "incompatible quote alias");
        let expected_balance = source.amount;
        source.amount = expected_balance
            .checked_sub(amount)
            .expect("insufficient quoted balance");
        source.needs_assertion = false;
        self.add_credit(to, definition, amount);
        token_call(
            config,
            from,
            &Message::TransferChecked {
                definition,
                expected_balance,
                to,
                amount,
            },
            seed,
        )
    }

    pub(crate) fn confirm(&self, config: &AmmConfig, mut response: Response) -> Response {
        for (&account, entry) in &self.0 {
            if entry.needs_assertion {
                response = response.send(token_call(
                    config,
                    account,
                    &Message::AssertHolding {
                        definition: entry.definition,
                        balance: entry.amount,
                    },
                    None,
                ));
            }
        }
        response
    }
}

pub(crate) fn transfer(
    checks: Option<&mut Balances>,
    config: &AmmConfig,
    from: AccountId,
    to: AccountId,
    definition: AccountId,
    amount: u128,
    seed: Option<PdaSeed>,
) -> SyncMessage {
    match checks {
        Some(checks) => checks.transfer(config, from, to, definition, amount, seed),
        None => token_call(config, from, &Message::Transfer { to, amount }, seed),
    }
}

/// The authenticated config reply is the only reply on these opted-in paths.
/// Quote holdings feed the same calculation as ordinary removal/add/swap/create.
/// Each quoted balance is paired with a settlement command or a Token assertion.
pub(crate) fn execute(
    input: &ReceiveInput,
    pending: &Pending,
    config: &AmmConfig,
) -> Option<Response> {
    if let AmmMessage::RemoveLiquidityChecked { request } = &pending.request {
        return Some(remove::execute_checked(
            input,
            reads::pool(pending),
            config,
            request,
        ));
    }

    let mut checks = Balances::default();
    let (request, pool, holdings, calculation) = match &pending.request {
        AmmMessage::NewDefinitionChecked { request } => (
            AmmMessage::NewDefinition {
                config: request.config,
                definition_token_a_id: request.definition_token_a_id,
                definition_token_b_id: request.definition_token_b_id,
                user_holding_a: request.user_holding_a,
                user_holding_b: request.user_holding_b,
                user_holding_lp: request.user_holding_lp,
                token_a_amount: request.token_a_amount,
                token_b_amount: request.token_b_amount,
                deadline: request.deadline,
            },
            None,
            vec![
                checks.remember(
                    request.user_holding_a,
                    request.definition_token_a_id,
                    request.source_a_balance,
                ),
                checks.remember(
                    request.user_holding_b,
                    request.definition_token_b_id,
                    request.source_b_balance,
                ),
            ],
            Calculation::Create,
        ),
        AmmMessage::AddLiquidityChecked { request } => {
            let mut pool = reads::pool(pending);
            let quote = &request.quote;
            pool.liquidity_pool_supply = quote.lp_supply;
            let mut holdings = vec![
                checks.remember(
                    request.user_holding_a,
                    pool.definition_token_a_id,
                    quote.source_a_balance,
                ),
                checks.remember(
                    request.user_holding_b,
                    pool.definition_token_b_id,
                    quote.source_b_balance,
                ),
                checks.remember(
                    request.user_holding_lp,
                    pool.liquidity_pool_id,
                    quote.recipient_lp_balance,
                ),
            ];
            holdings.extend(checks.vaults(&pool, &quote.pool));
            (
                AmmMessage::AddLiquidity {
                    user_holding_a: request.user_holding_a,
                    user_holding_b: request.user_holding_b,
                    user_holding_lp: request.user_holding_lp,
                    min_amount_liquidity: request.min_amount_liquidity,
                    max_amount_to_add_token_a: request.max_amount_to_add_token_a,
                    max_amount_to_add_token_b: request.max_amount_to_add_token_b,
                    deadline: request.deadline,
                },
                Some(pool),
                holdings,
                Calculation::Add((quote.amount_a, quote.amount_b, quote.liquidity)),
            )
        }
        AmmMessage::SwapExactInputChecked {
            user_input_holding,
            user_output_holding,
            swap_amount_in,
            min_amount_out,
            deadline,
            quote,
        } => {
            let pool = reads::pool(pending);
            let holdings = swap_holdings(
                &mut checks,
                &pool,
                *user_input_holding,
                *user_output_holding,
                quote,
            );
            (
                AmmMessage::SwapExactInput {
                    user_input_holding: *user_input_holding,
                    user_output_holding: *user_output_holding,
                    swap_amount_in: *swap_amount_in,
                    min_amount_out: *min_amount_out,
                    deadline: *deadline,
                },
                Some(pool),
                holdings,
                Calculation::Swap((quote.amount_in, quote.amount_out)),
            )
        }
        AmmMessage::SwapExactOutputChecked {
            user_input_holding,
            user_output_holding,
            exact_amount_out,
            max_amount_in,
            deadline,
            quote,
        } => {
            let pool = reads::pool(pending);
            let holdings = swap_holdings(
                &mut checks,
                &pool,
                *user_input_holding,
                *user_output_holding,
                quote,
            );
            (
                AmmMessage::SwapExactOutput {
                    user_input_holding: *user_input_holding,
                    user_output_holding: *user_output_holding,
                    exact_amount_out: *exact_amount_out,
                    max_amount_in: *max_amount_in,
                    deadline: *deadline,
                },
                Some(pool),
                holdings,
                Calculation::Swap((quote.amount_in, quote.amount_out)),
            )
        }
        AmmMessage::SyncReservesChecked { quote } => {
            maintenance(pending, &mut checks, quote, AmmMessage::SyncReserves)
        }
        AmmMessage::CreatePriceObservationsChecked {
            window_duration,
            quote,
        } => maintenance(
            pending,
            &mut checks,
            quote,
            AmmMessage::CreatePriceObservations {
                window_duration: *window_duration,
            },
        ),
        AmmMessage::CreateOraclePriceAccountChecked {
            window_duration,
            quote,
        } => maintenance(
            pending,
            &mut checks,
            quote,
            AmmMessage::CreateOraclePriceAccount {
                window_duration: *window_duration,
            },
        ),
        AmmMessage::Initialize { .. }
        | AmmMessage::UpdateConfig { .. }
        | AmmMessage::NewDefinition { .. }
        | AmmMessage::AddLiquidity { .. }
        | AmmMessage::RemoveLiquidity { .. }
        | AmmMessage::SwapExactInput { .. }
        | AmmMessage::SwapExactOutput { .. }
        | AmmMessage::SyncReserves
        | AmmMessage::CreatePriceObservations { .. }
        | AmmMessage::CreateOraclePriceAccount { .. }
        | AmmMessage::WithdrawProtocolFees { .. }
        | AmmMessage::ReadState
        | AmmMessage::RemoveLiquidityChecked { .. }
        | AmmMessage::WithdrawProtocolFeesChecked { .. } => return None,
    };
    if let Some(pool) = &pool {
        assert!(
            pool.liquidity_pool_supply >= MINIMUM_LIQUIDITY,
            "pool liquidity is below minimum"
        );
    }
    let calculation_input = Pending {
        config_id: pending.config_id,
        config: Some(config.clone()),
        pool,
        request,
        // The continuation is complete. These fields are unused by the calculation.
        expected_account: pending.expected_account,
        expected_program: pending.expected_program,
        remaining: vec![],
        holdings,
    };
    Some(match calculation {
        Calculation::Create => {
            new_definition::execute_with_checks(input, &calculation_input, Some(checks))
        }
        Calculation::Add(expected) => {
            add::execute_with_checks(input, &calculation_input, Some((checks, expected)))
        }
        Calculation::Swap(expected) => {
            swap::execute_with_checks(input, &calculation_input, Some((checks, expected)))
        }
        Calculation::Maintenance => sync::execute_with_preconditions(
            input,
            &calculation_input,
            checks.confirm(config, Response::keep_state()),
        ),
    })
}

enum Calculation {
    Create,
    Add((u128, u128, u128)),
    Swap((u128, u128)),
    Maintenance,
}

fn maintenance(
    pending: &Pending,
    checks: &mut Balances,
    quote: &PoolQuote,
    request: AmmMessage,
) -> (
    AmmMessage,
    Option<PoolDefinition>,
    Vec<Option<TokenHolding>>,
    Calculation,
) {
    let pool = reads::pool(pending);
    let holdings = checks.vaults(&pool, quote).into();
    (request, Some(pool), holdings, Calculation::Maintenance)
}

fn swap_holdings(
    checks: &mut Balances,
    pool: &PoolDefinition,
    user_input: AccountId,
    user_output: AccountId,
    quote: &amm_core::SwapQuote,
) -> Vec<Option<TokenHolding>> {
    let output_definition = if quote.input_definition == pool.definition_token_a_id {
        pool.definition_token_b_id
    } else {
        assert_eq!(
            quote.input_definition, pool.definition_token_b_id,
            "input token is not part of the pool"
        );
        pool.definition_token_a_id
    };
    let mut holdings = vec![
        checks.remember(user_input, quote.input_definition, quote.source_balance),
        checks.remember(user_output, output_definition, quote.recipient_balance),
    ];
    holdings.extend(checks.vaults(pool, &quote.pool));
    holdings
}
