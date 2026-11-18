//! AMM actor conversations through real AMM/token guests and a recording oracle mock.

use amm_core::{
    compute_config_pda, compute_liquidity_token_pda, compute_lp_lock_holding_pda, compute_pool_pda,
    compute_protocol_fee_pda, compute_vault_pda,
    event::Event,
    oracle,
    policy::{CalculationOp, CalculationPolicy, WithdrawalOp, WithdrawalPolicy},
    spot_price_q64_64, swap_exact_in_amounts, swap_exact_out_amounts, AmmConfig, AmmState, Message,
    PoolDefinition, MINIMUM_LIQUIDITY,
};
use integration_tests::{account, assert_refused, deploy, deploy_at, key, submit};
use lee::{error::LeeError, PrivateKey, V03State};
use lee_core::{
    account::{AccountId, Actor},
    program::TransactionEvent,
};
use oracle_mock_methods::oracle::OracleState;
use token_core::{Message as TokenMessage, TokenState};

const DEPOSIT_A: u128 = 1_000_000;
const DEPOSIT_B: u128 = 2_000_000;
const SUPPLY: u128 = 1_000_000_000;

struct Chain {
    state: V03State,
    amm: AccountId,
    token: AccountId,
    oracle: AccountId,
    config: AccountId,
    pool: AccountId,
    a: AccountId,
    b: AccountId,
    user_a: AccountId,
    user_b: AccountId,
    user_lp: AccountId,
}

impl Chain {
    fn empty(protocol_fee_bps: u128) -> Self {
        let (state, ids) = deploy(&[
            amm_methods::AMM_ELF,
            token_methods::TOKEN_ELF,
            oracle_mock_methods::ORACLE_MOCK_ELF,
        ]);
        let [amm, token, oracle] = ids.as_slice() else {
            panic!("three guests deployed")
        };
        Self::setup(state, *amm, *token, *oracle, protocol_fee_bps)
    }

    fn setup(
        state: V03State,
        amm: AccountId,
        token: AccountId,
        oracle: AccountId,
        protocol_fee_bps: u128,
    ) -> Self {
        let config = compute_config_pda(amm, account(&key(1)), [0; 32]);
        let (a, b) = (account(&key(2)), account(&key(3)));
        let mut chain = Self {
            state,
            amm,
            token,
            oracle,
            config,
            pool: compute_pool_pda(amm, config, a, b),
            a,
            b,
            user_a: account(&key(4)),
            user_b: account(&key(5)),
            user_lp: account(&key(6)),
        };
        for (definition, holding, definition_key, holding_key) in [
            (a, chain.user_a, key(2), key(4)),
            (b, chain.user_b, key(3), key(5)),
        ] {
            chain
                .token_submit(
                    definition,
                    &[definition, holding],
                    &[&definition_key, &holding_key],
                    &TokenMessage::NewDefinition {
                        name: "Test".to_owned(),
                        total_supply: SUPPLY,
                        mint_authority: None,
                        holding,
                    },
                )
                .expect("token created");
        }
        chain
            .call(
                account(&key(1)),
                &[&key(1)],
                &Message::Initialize {
                    nonce: [0; 32],
                    config: AmmConfig {
                        token_program_id: token,
                        twap_oracle_program_id: oracle,
                        authority: account(&key(1)),
                        swap_fee_bps: 30,
                        protocol_fee_bps,
                    },
                },
            )
            .expect("AMM initialized");
        chain
    }

    fn new() -> Self {
        let mut chain = Self::empty(2_000);
        chain.create().expect("pool created");
        chain
    }

    fn create_message(&self) -> Message {
        Message::NewDefinition {
            config: self.config,
            definition_token_a_id: self.a,
            definition_token_b_id: self.b,
            user_holding_a: self.user_a,
            user_holding_b: self.user_b,
            user_holding_lp: self.user_lp,
            token_a_amount: DEPOSIT_A,
            token_b_amount: DEPOSIT_B,
            deadline: u64::MAX,
        }
    }

    fn create(&mut self) -> Result<Vec<TransactionEvent>, LeeError> {
        self.call(
            self.config,
            &[&key(4), &key(5), &key(6)],
            &self.create_message(),
        )
    }

    fn actors(&self) -> Vec<Actor> {
        let mut actors = vec![
            Actor::new(self.config, self.amm),
            Actor::new(self.pool, self.amm),
            Actor::new(account(&key(1)), self.amm),
            Actor::new(self.pool, self.oracle),
        ];
        actors.extend(
            [
                self.a,
                self.b,
                self.user_a,
                self.user_b,
                self.user_lp,
                compute_vault_pda(self.amm, self.pool, self.a),
                compute_vault_pda(self.amm, self.pool, self.b),
                compute_liquidity_token_pda(self.amm, self.pool),
                compute_lp_lock_holding_pda(self.amm, self.pool),
                compute_protocol_fee_pda(self.amm, self.config, self.a),
                compute_protocol_fee_pda(self.amm, self.config, self.b),
            ]
            .into_iter()
            .map(|id| Actor::new(id, self.token)),
        );
        actors
    }

    fn call(
        &mut self,
        root: AccountId,
        signers: &[&PrivateKey],
        message: &Message,
    ) -> Result<Vec<TransactionEvent>, LeeError> {
        let mut actors = self.actors();
        let root = Actor::new(root, self.amm);
        if !actors.contains(&root) {
            actors.push(root);
        }
        submit(&mut self.state, root, &actors, signers, message)
    }

    fn token_submit(
        &mut self,
        root: AccountId,
        accounts: &[AccountId],
        signers: &[&PrivateKey],
        message: &TokenMessage,
    ) -> Result<Vec<TransactionEvent>, LeeError> {
        let actors: Vec<_> = accounts
            .iter()
            .map(|id| Actor::new(*id, self.token))
            .collect();
        submit(
            &mut self.state,
            Actor::new(root, self.token),
            &actors,
            signers,
            message,
        )
    }

    fn pool(&self) -> PoolDefinition {
        match AmmState::decode(
            self.state
                .get_account_by_id(self.pool)
                .data
                .actor_state(self.amm),
        ) {
            Some(AmmState::Pool(pool)) => pool,
            other => panic!("expected pool, got {other:?}"),
        }
    }

    fn balance(&self, id: AccountId) -> u128 {
        match TokenState::decode(
            self.state
                .get_account_by_id(id)
                .data
                .actor_state(self.token),
        ) {
            Some(TokenState::Holding(holding)) => holding.balance,
            other => panic!("expected holding, got {other:?}"),
        }
    }

    fn lp_supply(&self) -> u128 {
        match TokenState::decode(
            self.state
                .get_account_by_id(self.pool().liquidity_pool_id)
                .data
                .actor_state(self.token),
        ) {
            Some(TokenState::Definition(definition)) => definition.total_supply,
            other => panic!("expected LP definition, got {other:?}"),
        }
    }

    fn oracle(&self) -> OracleState {
        borsh::from_slice(
            self.state
                .get_account_by_id(self.pool)
                .data
                .actor_state(self.oracle),
        )
        .expect("mock oracle state decodes")
    }

    fn assert_reserves(&self) {
        let pool = self.pool();
        assert_eq!(self.balance(pool.vault_a_id), pool.reserve_a);
        assert_eq!(self.balance(pool.vault_b_id), pool.reserve_b);
        assert_eq!(self.lp_supply(), pool.liquidity_pool_supply);
        assert_eq!(
            self.oracle().price,
            spot_price_q64_64(pool.reserve_a, pool.reserve_b)
        );
        assert_eq!(
            self.balance(compute_lp_lock_holding_pda(self.amm, self.pool)),
            MINIMUM_LIQUIDITY
        );
    }

    fn swap_in(&self, a_to_b: bool, amount: u128) -> Message {
        let (source, target) = if a_to_b {
            (self.user_a, self.user_b)
        } else {
            (self.user_b, self.user_a)
        };
        Message::SwapExactInput {
            user_input_holding: source,
            user_output_holding: target,
            swap_amount_in: amount,
            min_amount_out: 1,
            deadline: u64::MAX,
        }
    }

    fn snapshot(&self) -> Vec<(Actor, Vec<u8>)> {
        self.actors()
            .into_iter()
            .map(|actor| {
                (
                    actor,
                    self.state
                        .get_account_by_id(actor.account_id)
                        .data
                        .actor_state(actor.program_account_id)
                        .to_vec(),
                )
            })
            .collect()
    }
}

#[test]
fn creation_initializes_vaults_lp_lock_and_authenticated_oracle() {
    let mut chain = Chain::empty(2_000);
    let events = chain.create().expect("create succeeds");
    let pool = chain.pool();
    assert_eq!((pool.reserve_a, pool.reserve_b), (DEPOSIT_A, DEPOSIT_B));
    assert_eq!(
        chain.balance(chain.user_lp),
        pool.liquidity_pool_supply
            .checked_sub(MINIMUM_LIQUIDITY)
            .expect("unlocked supply")
    );
    assert_eq!(chain.oracle().source, Actor::new(chain.pool, chain.amm));
    assert!(events.iter().any(|event| event.account_id == chain.amm
        && matches!(
            borsh::from_slice::<Event>(&event.event.data),
            Ok(Event::PoolCreated { .. })
        )));
    chain.assert_reserves();
    let before = chain.snapshot();
    assert_refused(&chain.create(), "pool already exists");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn exact_input_and_output_in_both_directions_preserve_reserves_and_protocol_fees() {
    let mut chain = Chain::new();
    for a_to_b in [true, false] {
        let pool = chain.pool();
        let (reserve_in, reserve_out, source, target, definition, signer) = if a_to_b {
            (
                pool.reserve_a,
                pool.reserve_b,
                chain.user_a,
                chain.user_b,
                chain.a,
                key(4),
            )
        } else {
            (
                pool.reserve_b,
                pool.reserve_a,
                chain.user_b,
                chain.user_a,
                chain.b,
                key(5),
            )
        };
        let amount = 10_000;
        let (effective, out) = swap_exact_in_amounts(amount, reserve_in, reserve_out, 30);
        let before_in = chain.balance(source);
        let before_out = chain.balance(target);
        chain
            .call(chain.pool, &[&signer], &chain.swap_in(a_to_b, amount))
            .expect("exact input succeeds");
        assert_eq!(
            chain.balance(source),
            before_in.checked_sub(amount).expect("debit")
        );
        assert_eq!(
            chain.balance(target),
            before_out.checked_add(out).expect("credit")
        );
        let fee = amm_core::protocol_fee_amount(amount.checked_sub(effective).expect("fee"), 2_000);
        let fee_id = compute_protocol_fee_pda(chain.amm, chain.config, definition);
        assert_eq!(chain.balance(fee_id), fee);
        chain.assert_reserves();
        let pool = chain.pool();
        let (rin, rout) = if a_to_b {
            (pool.reserve_a, pool.reserve_b)
        } else {
            (pool.reserve_b, pool.reserve_a)
        };
        let (_, required) = swap_exact_out_amounts(500, rin, rout, 30).expect("quote");
        let before = chain.balance(target);
        chain
            .call(
                chain.pool,
                &[&signer],
                &Message::SwapExactOutput {
                    user_input_holding: source,
                    user_output_holding: target,
                    exact_amount_out: 500,
                    max_amount_in: required,
                    deadline: u64::MAX,
                },
            )
            .expect("exact output succeeds");
        assert_eq!(
            chain.balance(target),
            before.checked_add(500).expect("credit")
        );
        chain.assert_reserves();
    }
}

#[test]
fn add_and_remove_liquidity_preserve_the_permanent_lock() {
    let mut chain = Chain::new();
    let initial_lp = chain.balance(chain.user_lp);
    chain
        .call(
            chain.pool,
            &[&key(4), &key(5)],
            &Message::AddLiquidity {
                user_holding_a: chain.user_a,
                user_holding_b: chain.user_b,
                user_holding_lp: chain.user_lp,
                min_amount_liquidity: 1,
                max_amount_to_add_token_a: 10_000,
                max_amount_to_add_token_b: 20_000,
                deadline: u64::MAX,
            },
        )
        .expect("add succeeds");
    assert!(chain.balance(chain.user_lp) > initial_lp);
    chain.assert_reserves();
    let liquidity = chain.balance(chain.user_lp);
    chain
        .call(
            chain.pool,
            &[&key(6)],
            &Message::RemoveLiquidity {
                user_holding_a: chain.user_a,
                user_holding_b: chain.user_b,
                user_holding_lp: chain.user_lp,
                remove_liquidity_amount: liquidity,
                min_amount_to_remove_token_a: 1,
                min_amount_to_remove_token_b: 1,
                deadline: u64::MAX,
            },
        )
        .expect("remove all unlocked liquidity");
    assert_eq!(chain.pool().liquidity_pool_supply, MINIMUM_LIQUIDITY);
    assert_eq!(chain.balance(chain.user_lp), 0);
    chain.assert_reserves();
}

fn calculation_policy(ops: &[CalculationOp]) -> CalculationPolicy {
    let mut policy = CalculationPolicy {
        ops: [CalculationOp::Nop; 32],
    };
    policy
        .ops
        .get_mut(..ops.len())
        .expect("test program fits")
        .copy_from_slice(ops);
    policy
}

#[test]
fn creation_policy_matches_canonical_supply_and_rejects_wrong_output() {
    use CalculationOp::{Emit, Input, SqrtProduct};

    let mut reference = Chain::empty(2_000);
    let mut chain = Chain::empty(2_000);
    let request = |policy| Message::NewDefinitionWithPolicy {
        config: chain.config,
        definition_token_a_id: chain.a,
        definition_token_b_id: chain.b,
        user_holding_a: chain.user_a,
        user_holding_b: chain.user_b,
        user_holding_lp: chain.user_lp,
        token_a_amount: DEPOSIT_A,
        token_b_amount: DEPOSIT_B,
        deadline: u64::MAX,
        policy,
    };
    let wrong = request(calculation_policy(&[Input { index: 0 }, Emit]));
    let matching = request(calculation_policy(&[
        Input { index: 0 },
        Input { index: 1 },
        SqrtProduct,
        Emit,
    ]));
    let before = chain.snapshot();
    assert_refused(
        &chain.call(chain.config, &[&key(4), &key(5), &key(6)], &wrong),
        "policy differs from canonical pool calculation",
    );
    assert_eq!(chain.snapshot(), before);
    chain
        .call(chain.config, &[&key(4), &key(5), &key(6)], &matching)
        .expect("matching supply calculation creates the pool");
    reference.create().expect("standard pool creation");
    assert_eq!(chain.snapshot(), reference.snapshot());
    chain.assert_reserves();
}

#[test]
fn add_policy_matches_deposits_and_mint_and_rejects_wrong_amounts() {
    use CalculationOp::{Emit, Input, Minimum, MulDivFloor, Output};

    let mut reference = Chain::new();
    let mut chain = Chain::new();
    let request = |policy| Message::AddLiquidityWithPolicy {
        user_holding_a: chain.user_a,
        user_holding_b: chain.user_b,
        user_holding_lp: chain.user_lp,
        min_amount_liquidity: 1,
        max_amount_to_add_token_a: 20_000,
        max_amount_to_add_token_b: 20_000,
        deadline: u64::MAX,
        policy,
    };
    let wrong = request(calculation_policy(&[
        Input { index: 2 },
        Emit,
        Input { index: 3 },
        Emit,
        Input { index: 4 },
        Emit,
    ]));
    let matching = request(calculation_policy(&[
        // min(max A, reserve A * max B / reserve B)
        Input { index: 2 },
        Input { index: 0 },
        Input { index: 3 },
        Input { index: 1 },
        MulDivFloor,
        Minimum,
        Emit,
        // min(max B, reserve B * max A / reserve A)
        Input { index: 3 },
        Input { index: 1 },
        Input { index: 2 },
        Input { index: 0 },
        MulDivFloor,
        Minimum,
        Emit,
        // min(supply * amount A / reserve A, supply * amount B / reserve B)
        Input { index: 4 },
        Output { index: 0 },
        Input { index: 0 },
        MulDivFloor,
        Input { index: 4 },
        Output { index: 1 },
        Input { index: 1 },
        MulDivFloor,
        Minimum,
        Emit,
    ]));
    let before = chain.snapshot();
    assert_refused(
        &chain.call(chain.pool, &[&key(4), &key(5)], &wrong),
        "policy differs from canonical pool calculation",
    );
    assert_eq!(chain.snapshot(), before);
    chain
        .call(chain.pool, &[&key(4), &key(5)], &matching)
        .expect("matching deposit and mint calculation");
    reference
        .call(
            reference.pool,
            &[&key(4), &key(5)],
            &Message::AddLiquidity {
                user_holding_a: reference.user_a,
                user_holding_b: reference.user_b,
                user_holding_lp: reference.user_lp,
                min_amount_liquidity: 1,
                max_amount_to_add_token_a: 20_000,
                max_amount_to_add_token_b: 20_000,
                deadline: u64::MAX,
            },
        )
        .expect("standard add liquidity");
    assert_eq!(chain.snapshot(), reference.snapshot());
    chain.assert_reserves();
}

#[test]
fn swap_policies_match_both_directions_and_reject_wrong_prices() {
    use CalculationOp::{
        Add, Emit, FeeDenominator, Input, MulDivCeil, MulDivFloor, Output, Subtract,
    };

    let exact_input = calculation_policy(&[
        // Effective input = floor(gross * (fee denominator - fee bps) / fee denominator).
        Input { index: 2 },
        FeeDenominator,
        Input { index: 3 },
        Subtract,
        FeeDenominator,
        MulDivFloor,
        Emit,
        // Output = floor(reserve out * effective input / (reserve in + effective input)).
        Input { index: 1 },
        Output { index: 0 },
        Input { index: 0 },
        Output { index: 0 },
        Add,
        MulDivFloor,
        Emit,
    ]);
    let exact_output = calculation_policy(&[
        // Effective input rounds up on the inverse curve.
        Input { index: 0 },
        Input { index: 2 },
        Input { index: 1 },
        Input { index: 2 },
        Subtract,
        MulDivCeil,
        Emit,
        // Gross input rounds up again to account for the fee.
        Output { index: 0 },
        FeeDenominator,
        FeeDenominator,
        Input { index: 3 },
        Subtract,
        MulDivCeil,
        Emit,
    ]);
    let wrong = calculation_policy(&[Input { index: 0 }, Emit, Input { index: 1 }, Emit]);
    let mut reference = Chain::new();
    let mut chain = Chain::new();
    for a_to_b in [true, false] {
        let (source, target, signer) = if a_to_b {
            (chain.user_a, chain.user_b, key(4))
        } else {
            (chain.user_b, chain.user_a, key(5))
        };
        let input_request = |policy| Message::SwapExactInputWithPolicy {
            user_input_holding: source,
            user_output_holding: target,
            swap_amount_in: 10_000,
            min_amount_out: 1,
            deadline: u64::MAX,
            policy,
        };
        let before = chain.snapshot();
        assert_refused(
            &chain.call(chain.pool, &[&signer], &input_request(wrong)),
            "policy differs from canonical pool calculation",
        );
        assert_eq!(chain.snapshot(), before);
        chain
            .call(chain.pool, &[&signer], &input_request(exact_input))
            .expect("matching exact input policy");
        reference
            .call(
                reference.pool,
                &[&signer],
                &reference.swap_in(a_to_b, 10_000),
            )
            .expect("standard exact input swap");
        assert_eq!(chain.snapshot(), reference.snapshot());
        chain.assert_reserves();

        let output_request = |policy| Message::SwapExactOutputWithPolicy {
            user_input_holding: source,
            user_output_holding: target,
            exact_amount_out: 750,
            max_amount_in: 10_000,
            deadline: u64::MAX,
            policy,
        };
        let before = chain.snapshot();
        assert_refused(
            &chain.call(chain.pool, &[&signer], &output_request(wrong)),
            "policy differs from canonical pool calculation",
        );
        assert_eq!(chain.snapshot(), before);
        chain
            .call(chain.pool, &[&signer], &output_request(exact_output))
            .expect("matching exact output policy");
        reference
            .call(
                reference.pool,
                &[&signer],
                &Message::SwapExactOutput {
                    user_input_holding: source,
                    user_output_holding: target,
                    exact_amount_out: 750,
                    max_amount_in: 10_000,
                    deadline: u64::MAX,
                },
            )
            .expect("standard exact output swap");
        assert_eq!(chain.snapshot(), reference.snapshot());
        chain.assert_reserves();
    }
}

fn withdrawal_policy(first_input: WithdrawalOp) -> WithdrawalPolicy {
    use WithdrawalOp::{Emit, Liquidity, MulDivFloor, Nop, ReserveB, Supply};
    WithdrawalPolicy {
        ops: [
            first_input,
            Liquidity,
            Supply,
            MulDivFloor,
            Emit,
            ReserveB,
            Liquidity,
            Supply,
            MulDivFloor,
            Emit,
            Nop,
            Nop,
            Nop,
            Nop,
            Nop,
            Nop,
        ],
    }
}

fn policy_message(chain: &Chain, liquidity: u128, policy: WithdrawalPolicy) -> Message {
    Message::RemoveLiquidityWithPolicy {
        user_holding_a: chain.user_a,
        user_holding_b: chain.user_b,
        user_holding_lp: chain.user_lp,
        remove_liquidity_amount: liquidity,
        min_amount_to_remove_token_a: 1,
        min_amount_to_remove_token_b: 1,
        deadline: u64::MAX,
        policy,
    }
}

#[test]
fn withdrawal_policy_uses_live_lp_supply_and_recorded_reserves() {
    let mut chain = Chain::new();
    let pool = chain.pool();
    let liquidity = 100_001;
    let external_burn = 100_000;
    let donation = 123;
    chain
        .token_submit(
            chain.user_lp,
            &[chain.user_lp, pool.liquidity_pool_id],
            &[&key(6)],
            &TokenMessage::Burn {
                amount: external_burn,
            },
        )
        .expect("an external LP burn changes the Token supply");
    chain
        .token_submit(
            chain.user_a,
            &[chain.user_a, pool.vault_a_id],
            &[&key(4)],
            &TokenMessage::Transfer {
                to: pool.vault_a_id,
                amount: donation,
            },
        )
        .expect("a direct donation leaves the recorded reserve unchanged");

    let supply = pool
        .liquidity_pool_supply
        .checked_sub(external_burn)
        .expect("external burn reduces supply");
    assert_eq!(chain.lp_supply(), supply);
    let (payout_a, payout_b) = (76_091, 152_183);
    let before_a = chain.balance(chain.user_a);
    let before_b = chain.balance(chain.user_b);
    let before_lp = chain.balance(chain.user_lp);
    let previous_oracle_price = chain.oracle().price;
    let request = policy_message(&chain, liquidity, withdrawal_policy(WithdrawalOp::ReserveA));
    chain
        .call(chain.pool, &[&key(6)], &request)
        .expect("a matching policy settles through the AMM");

    let next = chain.pool();
    assert_eq!(
        chain.balance(chain.user_a),
        before_a.checked_add(payout_a).expect("A credit")
    );
    assert_eq!(
        chain.balance(chain.user_b),
        before_b.checked_add(payout_b).expect("B credit")
    );
    assert_eq!(
        chain.balance(chain.user_lp),
        before_lp.checked_sub(liquidity).expect("LP debit")
    );
    assert_eq!(
        next.reserve_a,
        pool.reserve_a.checked_sub(payout_a).expect("A reserve")
    );
    assert_eq!(
        next.reserve_b,
        pool.reserve_b.checked_sub(payout_b).expect("B reserve")
    );
    assert_eq!(
        chain.balance(next.vault_a_id),
        next.reserve_a
            .checked_add(donation)
            .expect("unsynced donation")
    );
    assert_eq!(chain.balance(next.vault_b_id), next.reserve_b);
    assert_eq!(
        chain.lp_supply(),
        supply.checked_sub(liquidity).expect("LP burn")
    );
    assert_eq!(chain.lp_supply(), next.liquidity_pool_supply);
    assert_eq!(
        chain.balance(compute_lp_lock_holding_pda(chain.amm, chain.pool)),
        MINIMUM_LIQUIDITY
    );
    assert_eq!(
        chain.oracle().price,
        spot_price_q64_64(next.reserve_a, next.reserve_b)
    );
    assert_ne!(
        chain.oracle().price,
        previous_oracle_price,
        "test should distinguish the oracle update"
    );
}

#[test]
fn withdrawal_policy_rejects_invalid_or_nonproportional_programs() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    for (policy, failure) in [
        (
            withdrawal_policy(WithdrawalOp::ReserveB),
            "policy differs from proportional withdrawal",
        ),
        (
            withdrawal_policy(WithdrawalOp::Nop),
            "policy stack underflow",
        ),
        (
            WithdrawalPolicy {
                ops: [WithdrawalOp::ReserveA; 16],
            },
            "policy stack limit exceeded",
        ),
    ] {
        let result = chain.call(
            chain.pool,
            &[&key(6)],
            &policy_message(&chain, 100_000, policy),
        );
        assert_refused(&result, failure);
        assert_eq!(chain.snapshot(), before);
    }

    let valid = policy_message(&chain, 100_000, withdrawal_policy(WithdrawalOp::ReserveA));
    let unsigned = chain.call(chain.pool, &[], &valid);
    assert_refused(&unsigned, "holding must authorize the burn");
    assert_eq!(chain.snapshot(), before);
    chain
        .call(chain.pool, &[&key(6)], &valid)
        .expect("failed calls did not leave the pool busy");
}

#[test]
fn direct_lp_burns_reduce_the_supply_used_to_mint_new_liquidity() {
    let mut chain = Chain::new();
    let pool = chain.pool();
    let burned = pool.liquidity_pool_supply / 2;
    chain
        .token_submit(
            chain.user_lp,
            &[chain.user_lp, pool.liquidity_pool_id],
            &[&key(6)],
            &TokenMessage::Burn { amount: burned },
        )
        .expect("LP holder can burn directly through token");
    let actual_supply = chain.lp_supply();
    assert_eq!(
        actual_supply,
        pool.liquidity_pool_supply
            .checked_sub(burned)
            .expect("burned supply")
    );
    let before_lp = chain.balance(chain.user_lp);
    // Doubling reserves should mint exactly the current outstanding supply,
    // preserving the donation from the direct burn for existing LP holders.
    chain
        .call(
            chain.pool,
            &[&key(4), &key(5)],
            &Message::AddLiquidity {
                user_holding_a: chain.user_a,
                user_holding_b: chain.user_b,
                user_holding_lp: chain.user_lp,
                min_amount_liquidity: actual_supply,
                max_amount_to_add_token_a: pool.reserve_a,
                max_amount_to_add_token_b: pool.reserve_b,
                deadline: u64::MAX,
            },
        )
        .expect("add uses the post-burn supply");
    assert_eq!(
        chain
            .balance(chain.user_lp)
            .checked_sub(before_lp)
            .expect("minted LP"),
        actual_supply
    );
    chain.assert_reserves();
}

#[test]
fn direct_lp_burns_donate_to_remaining_liquidity_holders() {
    let mut chain = Chain::new();
    let pool = chain.pool();
    chain
        .token_submit(
            chain.user_lp,
            &[chain.user_lp, pool.liquidity_pool_id],
            &[&key(6)],
            &TokenMessage::Burn {
                amount: pool.liquidity_pool_supply / 2,
            },
        )
        .expect("direct LP burn succeeds");
    let liquidity = chain.balance(chain.user_lp);
    let supply = chain.lp_supply();
    let expected_a = amm_core::mul_div_floor(pool.reserve_a, liquidity, supply);
    let expected_b = amm_core::mul_div_floor(pool.reserve_b, liquidity, supply);
    let (before_a, before_b) = (chain.balance(chain.user_a), chain.balance(chain.user_b));
    chain
        .call(
            chain.pool,
            &[&key(6)],
            &Message::RemoveLiquidity {
                user_holding_a: chain.user_a,
                user_holding_b: chain.user_b,
                user_holding_lp: chain.user_lp,
                remove_liquidity_amount: liquidity,
                min_amount_to_remove_token_a: expected_a,
                min_amount_to_remove_token_b: expected_b,
                deadline: u64::MAX,
            },
        )
        .expect("remaining holder withdraws the donated share");
    assert_eq!(
        chain.balance(chain.user_a),
        before_a.checked_add(expected_a).expect("A credit")
    );
    assert_eq!(
        chain.balance(chain.user_b),
        before_b.checked_add(expected_b).expect("B credit")
    );
    assert_eq!(chain.balance(chain.user_lp), 0);
    assert_eq!(chain.pool().liquidity_pool_supply, MINIMUM_LIQUIDITY);
    chain.assert_reserves();
}

#[test]
fn swaps_refuse_missing_signatures_slippage_wrong_tokens_and_undeclared_vaults_atomically() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    assert_refused(
        &chain.call(chain.pool, &[], &chain.swap_in(true, 10_000)),
        "sender holding must authorize the transfer",
    );
    let mut excessive_min = chain.swap_in(true, 10_000);
    if let Message::SwapExactInput { min_amount_out, .. } = &mut excessive_min {
        *min_amount_out = u128::MAX;
    }
    assert_refused(
        &chain.call(chain.pool, &[&key(4)], &excessive_min),
        "output below minimum",
    );
    let mut wrong_target = chain.swap_in(true, 10_000);
    if let Message::SwapExactInput {
        user_output_holding,
        ..
    } = &mut wrong_target
    {
        *user_output_holding = chain.user_a;
    }
    assert_refused(
        &chain.call(chain.pool, &[&key(4)], &wrong_target),
        "holding has the wrong token definition",
    );
    let mut actors = chain.actors();
    actors.retain(|a| *a != Actor::new(chain.pool().vault_a_id, chain.token));
    let message = chain.swap_in(true, 10_000);
    let result = submit(
        &mut chain.state,
        Actor::new(chain.pool, chain.amm),
        &actors,
        &[&key(4)],
        &message,
    );
    assert!(
        result.is_err(),
        "vault must be declared to guarantee atomic credit"
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn failed_creation_rolls_back_earlier_token_initialization_and_namespace_is_signed() {
    let mut chain = Chain::empty(0);
    let before = chain.snapshot();
    // A and LP sign, B does not. The guest reaches B only after creating the LP and vaults.
    assert_refused(
        &chain.call(chain.config, &[&key(4), &key(6)], &chain.create_message()),
        "sender holding must authorize the transfer",
    );
    assert_eq!(chain.snapshot(), before);
    let result = chain.call(
        account(&key(1)),
        &[],
        &Message::Initialize {
            nonce: [1; 32],
            config: AmmConfig {
                token_program_id: chain.token,
                twap_oracle_program_id: chain.oracle,
                authority: account(&key(1)),
                swap_fee_bps: 30,
                protocol_fee_bps: 0,
            },
        },
    );
    assert_refused(&result, "AMM owner or authority must authorize");
    chain
        .create()
        .expect("failed creation left no pending state");
    chain.assert_reserves();
}

#[test]
fn donations_are_synchronized_and_oracle_feeds_use_pool_prices() {
    let mut chain = Chain::new();
    let vault = chain.pool().vault_a_id;
    chain
        .token_submit(
            chain.user_a,
            &[chain.user_a, vault],
            &[&key(4)],
            &TokenMessage::Transfer {
                to: vault,
                amount: 123,
            },
        )
        .expect("donation");
    assert_eq!(chain.pool().reserve_a, DEPOSIT_A);
    chain
        .call(chain.pool, &[], &Message::SyncReserves)
        .expect("sync");
    assert_eq!(
        chain.pool().reserve_a,
        DEPOSIT_A.checked_add(123).expect("donation")
    );
    chain.assert_reserves();
    chain
        .call(
            chain.pool,
            &[],
            &Message::CreatePriceObservations {
                window_duration: 86_400_000,
            },
        )
        .expect("observations");
    chain
        .call(
            chain.pool,
            &[],
            &Message::CreateOraclePriceAccount {
                window_duration: 86_400_000,
            },
        )
        .expect("price account");
    let oracle = chain.oracle();
    assert_eq!(oracle.observations, vec![86_400_000]);
    assert_eq!(
        oracle.prices,
        vec![oracle::Message::CreateOraclePriceAccount {
            base_asset: chain.a,
            quote_asset: chain.b,
            initial_price: spot_price_q64_64(chain.pool().reserve_a, chain.pool().reserve_b),
            window_duration: 86_400_000
        }]
    );
    let before = chain.snapshot();
    assert_refused(
        &chain.call(
            chain.pool,
            &[],
            &Message::CreatePriceObservations {
                window_duration: 86_400_000,
            },
        ),
        "observations already exist",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn oracle_price_policies_match_their_reserve_sources_and_reject_reversed_prices() {
    use CalculationOp::{Emit, Input, SpotPriceQ64_64};

    let mut reference = Chain::new();
    let mut chain = Chain::new();
    let window_duration = 86_400_000;
    for chain in [&mut reference, &mut chain] {
        let vault = chain.pool().vault_a_id;
        chain
            .token_submit(
                chain.user_a,
                &[chain.user_a, vault],
                &[&key(4)],
                &TokenMessage::Transfer {
                    to: vault,
                    amount: 123,
                },
            )
            .expect("donation");
        chain
            .call(
                chain.pool,
                &[],
                &Message::CreatePriceObservations { window_duration },
            )
            .expect("observations");
    }

    let matching = calculation_policy(&[
        Input { index: 0 },
        Input { index: 1 },
        SpotPriceQ64_64,
        Emit,
    ]);
    let reversed = calculation_policy(&[
        Input { index: 1 },
        Input { index: 0 },
        SpotPriceQ64_64,
        Emit,
    ]);
    let create = |policy| Message::CreateOraclePriceAccountWithPolicy {
        window_duration,
        policy,
    };
    let before = chain.snapshot();
    assert_refused(
        &chain.call(chain.pool, &[], &create(reversed)),
        "policy differs from canonical pool calculation",
    );
    assert_eq!(chain.snapshot(), before);
    chain
        .call(chain.pool, &[], &create(matching))
        .expect("matching price account calculation");
    reference
        .call(
            reference.pool,
            &[],
            &Message::CreateOraclePriceAccount { window_duration },
        )
        .expect("standard price account creation");
    assert_eq!(chain.snapshot(), reference.snapshot());
    assert_eq!(
        chain.oracle().prices,
        vec![oracle::Message::CreateOraclePriceAccount {
            base_asset: chain.a,
            quote_asset: chain.b,
            initial_price: spot_price_q64_64(DEPOSIT_A, DEPOSIT_B),
            window_duration,
        }]
    );

    let sync = |policy| Message::SyncReservesWithPolicy { policy };
    let before = chain.snapshot();
    assert_refused(
        &chain.call(chain.pool, &[], &sync(reversed)),
        "policy differs from canonical pool calculation",
    );
    assert_eq!(chain.snapshot(), before);
    let events = chain
        .call(chain.pool, &[], &sync(matching))
        .expect("matching post-sync price calculation");
    reference
        .call(reference.pool, &[], &Message::SyncReserves)
        .expect("standard reserve synchronization");
    assert_eq!(chain.snapshot(), reference.snapshot());
    let synced_a = DEPOSIT_A + 123;
    assert!(events.iter().any(|event| event.account_id == chain.amm
        && matches!(
            borsh::from_slice::<Event>(&event.event.data),
            Ok(Event::ReservesSynced { reserve_a, reserve_b })
                if reserve_a == synced_a && reserve_b == DEPOSIT_B
        )));
    assert_eq!(chain.oracle().price, spot_price_q64_64(synced_a, DEPOSIT_B));
    chain.assert_reserves();
}

#[test]
fn protocol_fee_withdrawal_follows_rotated_authority() {
    let mut chain = Chain::new();
    chain
        .call(chain.pool, &[&key(4)], &chain.swap_in(true, 10_000))
        .expect("swap");
    let fees = compute_protocol_fee_pda(chain.amm, chain.config, chain.a);
    let amount = chain.balance(fees);
    let withdraw = Message::WithdrawProtocolFees {
        config: chain.config,
        token: chain.a,
        destination: chain.user_a,
        amount,
    };
    assert_refused(
        &chain.call(account(&key(7)), &[&key(7)], &withdraw),
        "sender is not the AMM admin authority",
    );
    chain
        .call(
            account(&key(1)),
            &[&key(1)],
            &Message::UpdateConfig {
                config: chain.config,
                new_authority: account(&key(7)),
            },
        )
        .expect("rotate");
    assert_refused(
        &chain.call(account(&key(1)), &[&key(1)], &withdraw),
        "sender is not the AMM admin authority",
    );
    let before = chain.balance(chain.user_a);
    chain
        .call(account(&key(7)), &[&key(7)], &withdraw)
        .expect("new authority withdraws");
    assert_eq!(chain.balance(fees), 0);
    assert_eq!(
        chain.balance(chain.user_a),
        before.checked_add(amount).expect("credit")
    );
    chain.assert_reserves();
}

#[test]
fn non_image_program_addresses_are_used_for_pdas_and_dispatch() {
    let (amm, token, oracle) = (
        AccountId::new([81; 32]),
        AccountId::new([82; 32]),
        AccountId::new([83; 32]),
    );
    let state = deploy_at(&[
        (amm, amm_methods::AMM_ELF),
        (token, token_methods::TOKEN_ELF),
        (oracle, oracle_mock_methods::ORACLE_MOCK_ELF),
    ]);
    let mut chain = Chain::setup(state, amm, token, oracle, 0);
    chain.create().expect("create at named deployments");
    chain
        .call(chain.pool, &[&key(5)], &chain.swap_in(false, 10_000))
        .expect("swap at named deployments");
    chain.assert_reserves();
}

#[test]
fn oracle_failure_rolls_back_transfers_fees_and_pool_state() {
    let mut chain = Chain::new();
    let mut oracle = chain.oracle();
    oracle.reject_updates = true;
    let mut account = chain.state.get_account_by_id(chain.pool);
    account.data.set_actor_state(
        chain.oracle,
        borsh::to_vec(&oracle).expect("mock state encodes").into(),
    );
    // Inject a downstream failure into the mock, without changing AMM or token state.
    chain.state.force_insert_account(chain.pool, account);
    let before = chain.snapshot();
    assert_refused(
        &chain.call(chain.pool, &[&key(4)], &chain.swap_in(true, 10_000)),
        "mock oracle refused update",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn permanent_lock_cannot_be_spent_by_removal_or_direct_token_messages() {
    let mut chain = Chain::new();
    let lock = compute_lp_lock_holding_pda(chain.amm, chain.pool);
    let before = chain.snapshot();
    let remove = Message::RemoveLiquidity {
        user_holding_a: chain.user_a,
        user_holding_b: chain.user_b,
        user_holding_lp: lock,
        remove_liquidity_amount: MINIMUM_LIQUIDITY,
        min_amount_to_remove_token_a: 1,
        min_amount_to_remove_token_b: 1,
        deadline: u64::MAX,
    };
    assert_refused(
        &chain.call(chain.pool, &[&key(1)], &remove),
        "holding must authorize the burn",
    );
    assert_refused(
        &chain.token_submit(
            lock,
            &[lock, chain.user_lp],
            &[&key(1)],
            &TokenMessage::Transfer {
                to: chain.user_lp,
                amount: 1,
            },
        ),
        "sender holding must authorize the transfer",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn expired_swaps_roll_back_and_the_deadline_itself_is_valid() {
    use lee::{
        public_transaction::{Message as TransactionMessage, WitnessSet},
        PublicTransaction,
    };
    let mut chain = Chain::new();
    let mut message = chain.swap_in(true, 10_000);
    if let Message::SwapExactInput { deadline, .. } = &mut message {
        *deadline = 50;
    }
    let signer = key(4);
    let before = chain.snapshot();
    for timestamp in [51, 50] {
        let tx = TransactionMessage::try_new(
            Actor::new(chain.pool, chain.amm),
            chain.actors(),
            [(
                account(&signer),
                chain.state.get_account_by_id(account(&signer)).nonce,
            )]
            .into(),
            &message,
        )
        .expect("transaction encodes");
        let witness = WitnessSet::for_message(&tx, &[&signer]);
        let result = chain.state.transition_from_public_transaction(
            &PublicTransaction::new(tx, witness),
            1,
            timestamp,
        );
        if timestamp == 51 {
            assert!(result.is_err(), "expired transaction must fail");
            assert_eq!(chain.snapshot(), before);
        } else {
            result.expect("deadline is inclusive");
        }
    }
    chain.assert_reserves();
}

#[test]
fn namespaces_keep_distinct_pools_and_reject_cross_config_creation() {
    let mut chain = Chain::new();
    let first_pool = chain.pool;
    let first_config = chain.config;
    let first_pool_state = chain.pool();
    let nonce = [1; 32];
    chain.config = compute_config_pda(chain.amm, account(&key(1)), nonce);
    chain.pool = compute_pool_pda(chain.amm, chain.config, chain.a, chain.b);
    chain.user_lp = account(&key(8));
    chain
        .call(
            account(&key(1)),
            &[&key(1)],
            &Message::Initialize {
                nonce,
                config: AmmConfig {
                    token_program_id: chain.token,
                    twap_oracle_program_id: chain.oracle,
                    authority: account(&key(1)),
                    swap_fee_bps: 5,
                    protocol_fee_bps: 0,
                },
            },
        )
        .expect("second namespace");
    let mut wrong_config = chain.create_message();
    if let Message::NewDefinition { config, .. } = &mut wrong_config {
        *config = first_config;
    }
    assert_refused(
        &chain.call(chain.config, &[&key(4), &key(5), &key(8)], &wrong_config),
        "pool config does not match receiver",
    );
    chain
        .call(
            chain.config,
            &[&key(4), &key(5), &key(8)],
            &chain.create_message(),
        )
        .expect("second pool");
    assert_ne!(chain.pool, first_pool);
    assert_ne!(chain.pool().vault_a_id, first_pool_state.vault_a_id);
    chain
        .call(chain.pool, &[&key(4)], &chain.swap_in(true, 10_000))
        .expect("second pool swap");
    assert_eq!(
        AmmState::decode(
            chain
                .state
                .get_account_by_id(first_pool)
                .data
                .actor_state(chain.amm)
        ),
        Some(AmmState::Pool(first_pool_state))
    );
    chain.assert_reserves();
}

#[test]
fn creation_checks_token_id_arguments_and_the_initial_lp_signature() {
    let mut chain = Chain::empty(0);
    let before = chain.snapshot();
    let result = chain.call(chain.config, &[&key(4), &key(5)], &chain.create_message());
    assert!(
        result.is_err(),
        "the initial LP holding did not authorize: {result:?}"
    );
    let mut wrong_pair = chain.create_message();
    // Swap the supplied definitions while keeping the holdings in their original order.
    // The sorted pair derives the same pool, so only authenticated holding reads catch it.
    if let Message::NewDefinition {
        definition_token_a_id,
        definition_token_b_id,
        ..
    } = &mut wrong_pair
    {
        std::mem::swap(definition_token_a_id, definition_token_b_id);
    }
    assert_refused(
        &chain.call(chain.config, &[&key(4), &key(5), &key(6)], &wrong_pair),
        "token A definition mismatch",
    );
    assert_eq!(chain.snapshot(), before);
}
