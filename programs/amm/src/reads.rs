use amm_core::{AmmConfig, AmmState, Message, Pending, PoolDefinition, MINIMUM_LIQUIDITY};
use lee_core::{
    account::{AccountId, Actor},
    program::{ReceiveInput, Response},
};
use token_core::TokenHolding;

use crate::{
    actor, add, config,
    coordinator::{ReadStep, ReadValue},
    new_definition, remove, swap, sync,
};

pub(crate) fn start(input: &ReceiveInput, state: Option<AmmState>, request: Message) -> Response {
    let (config_id, pool) = if let Message::NewDefinition { config, .. } = &request {
        assert!(state.is_none(), "pool already exists");
        (*config, None)
    } else {
        let Some(AmmState::Pool(pool)) = state else {
            panic!("receiver is not an AMM pool")
        };
        (pool.config_id, Some(pool))
    };
    let deadline = match &request {
        Message::NewDefinition { deadline, .. }
        | Message::AddLiquidity { deadline, .. }
        | Message::RemoveLiquidity { deadline, .. }
        | Message::SwapExactInput { deadline, .. }
        | Message::SwapExactOutput { deadline, .. } => Some(*deadline),
        Message::SyncReserves
        | Message::CreatePriceObservations { .. }
        | Message::CreateOraclePriceAccount { .. } => None,
        Message::Initialize { .. }
        | Message::UpdateConfig { .. }
        | Message::WithdrawProtocolFees { .. }
        | Message::ReadState => unreachable!("pool operation"),
    };
    let expected = actor(input, config_id);
    let pending = Pending {
        config_id,
        config: None,
        pool,
        request,
        expected_account: expected.account_id,
        expected_program: expected.program_account_id,
        remaining: vec![],
        holdings: vec![],
    };
    let response = ReadStep::Config.ask(expected, pending);
    // The former clock check accepted timestamp == deadline. An unbounded upper end
    // represents u64::MAX without overflowing its exclusive endpoint.
    match deadline.and_then(|value| value.checked_add(1)) {
        Some(end) => response.timestamp_window(..end),
        None => response,
    }
}

pub(crate) fn resume(input: &ReceiveInput, mut pending: Pending, step: ReadStep) -> Response {
    match step.receive(input, &pending) {
        ReadValue::Config(config) => config_reply(input, pending, config),
        ReadValue::Holding(holding) => {
            pending.holdings.push(holding);
            next(input, pending)
        }
        ReadValue::LpSupply(supply) => {
            pending
                .pool
                .as_mut()
                .expect("liquidity operation has a pool")
                .liquidity_pool_supply = supply;
            execute(input, &pending)
        }
    }
}

fn config_reply(input: &ReceiveInput, mut pending: Pending, config: AmmConfig) -> Response {
    pending.config = Some(config);
    let mut accounts = match &pending.request {
        Message::NewDefinition {
            user_holding_a,
            user_holding_b,
            ..
        } => vec![*user_holding_a, *user_holding_b],
        Message::AddLiquidity {
            user_holding_a,
            user_holding_b,
            user_holding_lp,
            ..
        }
        | Message::RemoveLiquidity {
            user_holding_a,
            user_holding_b,
            user_holding_lp,
            ..
        } => vec![*user_holding_a, *user_holding_b, *user_holding_lp],
        Message::SwapExactInput {
            user_input_holding,
            user_output_holding,
            ..
        }
        | Message::SwapExactOutput {
            user_input_holding,
            user_output_holding,
            ..
        } => vec![*user_input_holding, *user_output_holding],
        Message::SyncReserves
        | Message::CreatePriceObservations { .. }
        | Message::CreateOraclePriceAccount { .. } => vec![],
        Message::Initialize { .. }
        | Message::UpdateConfig { .. }
        | Message::WithdrawProtocolFees { .. }
        | Message::ReadState => unreachable!("pool operation"),
    };
    if let Some(pool) = &pending.pool {
        accounts.extend([pool.vault_a_id, pool.vault_b_id]);
    }
    holdings(input, pending, accounts)
}

pub(crate) fn holdings(
    input: &ReceiveInput,
    mut pending: Pending,
    mut accounts: Vec<AccountId>,
) -> Response {
    accounts.reverse();
    pending.remaining = accounts;
    next(input, pending)
}

fn next(input: &ReceiveInput, mut pending: Pending) -> Response {
    if let Some(account) = pending.remaining.pop() {
        let token = pending
            .config
            .as_ref()
            .expect("config was read")
            .token_program_id;
        return ReadStep::Holding.ask(Actor::new(account, token), pending);
    }
    if let Some(pool) = &pending.pool {
        let [.., a, b] = pending.holdings.as_slice() else {
            unreachable!("vaults were read")
        };
        assert!(
            holding(a, pool.definition_token_a_id).balance >= pool.reserve_a,
            "vault A is below reserve"
        );
        assert!(
            holding(b, pool.definition_token_b_id).balance >= pool.reserve_b,
            "vault B is below reserve"
        );
    }
    if matches!(
        pending.request,
        Message::AddLiquidity { .. } | Message::RemoveLiquidity { .. }
    ) {
        let pool = pending
            .pool
            .as_ref()
            .expect("liquidity operation has a pool");
        let token = pending
            .config
            .as_ref()
            .expect("config was read")
            .token_program_id;
        let expected = Actor::new(pool.liquidity_pool_id, token);
        // LP holders may burn directly through token. Read its authoritative supply
        // without granting the mint-authority seed to this read-only operation.
        return ReadStep::LpSupply.ask(expected, pending);
    }
    execute(input, &pending)
}

fn execute(input: &ReceiveInput, pending: &Pending) -> Response {
    if let Some(pool) = &pending.pool {
        assert!(
            pool.liquidity_pool_supply >= MINIMUM_LIQUIDITY,
            "pool liquidity is below minimum"
        );
    }
    match pending.request {
        Message::NewDefinition { .. } => new_definition::execute(input, pending),
        Message::AddLiquidity { .. } => add::execute(input, pending),
        Message::RemoveLiquidity { .. } => remove::execute(input, pending),
        Message::SwapExactInput { .. } | Message::SwapExactOutput { .. } => {
            swap::execute(input, pending)
        }
        Message::SyncReserves
        | Message::CreatePriceObservations { .. }
        | Message::CreateOraclePriceAccount { .. } => sync::execute(input, pending),
        Message::WithdrawProtocolFees { .. } => config::withdraw(input, pending),
        Message::Initialize { .. } | Message::UpdateConfig { .. } | Message::ReadState => {
            unreachable!("operation continuation")
        }
    }
}

pub(crate) fn holding(snapshot: &Option<TokenHolding>, definition: AccountId) -> &TokenHolding {
    let holding = snapshot
        .as_ref()
        .expect("token holding must be initialized");
    assert_eq!(
        holding.definition_id, definition,
        "holding has the wrong token definition"
    );
    holding
}

pub(crate) fn pool(pending: &Pending) -> PoolDefinition {
    pending.pool.clone().expect("pool was loaded")
}
