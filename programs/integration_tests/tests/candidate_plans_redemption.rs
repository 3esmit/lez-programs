use integration_tests::{account, assert_refused, deploy_at, key, submit};
use lee::{error::LeeError, V03State};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::TransactionEvent,
};
use token_candidate_plans_example::redemption::{
    self, Candidate, Command, Config, Phase, Redeem, State,
};
use token_core::{Message, TokenState};

const TOKEN: AccountId = AccountId::new([0xa0; 32]);
const REDEMPTION: AccountId = AccountId::new([0xb0; 32]);
fn id(byte: u8) -> AccountId {
    account(&key(byte))
}
fn token(account: AccountId) -> Actor {
    Actor::new(account, TOKEN)
}
fn processor() -> Actor {
    Actor::new(id(4), REDEMPTION)
}
fn vault(asset: u8) -> AccountId {
    redemption::vault(processor(), id(asset))
}

struct Chain {
    state: V03State,
}
impl Chain {
    fn new() -> Self {
        let mut state = deploy_at(&[
            (TOKEN, token_methods::TOKEN_ELF),
            (
                REDEMPTION,
                token_candidate_plans_example::CANDIDATE_PLANS_REDEMPTION_ELF,
            ),
        ]);
        for (definition, holding, supply) in [(11, 12, 10_000), (13, 14, 20_000), (15, 2, 1_000)] {
            submit(
                &mut state,
                token(id(definition)),
                &[token(id(definition)), token(id(holding))],
                &[&key(definition), &key(holding)],
                &Message::NewDefinition {
                    name: format!("Asset {definition}"),
                    total_supply: supply,
                    mint_authority: None,
                    holding: id(holding),
                },
            )
            .expect("create asset");
        }
        for (holding, definition) in [(21, 11), (22, 13)] {
            submit(
                &mut state,
                token(id(holding)),
                &[token(id(holding)), token(id(definition))],
                &[&key(holding)],
                &Message::InitializeHolding {
                    definition: id(definition),
                },
            )
            .expect("initialize output holding");
        }
        submit(
            &mut state,
            processor(),
            &[
                processor(),
                token(vault(11)),
                token(vault(13)),
                token(id(11)),
                token(id(13)),
            ],
            &[&key(4)],
            &Command::Initialize(Config {
                token_program: TOKEN,
                asset_a: id(11),
                asset_b: id(13),
                shares: id(15),
            }),
        )
        .expect("initialize redemption and reserve accounts");
        let mut chain = Self { state };
        chain.donate(11, 12, 1_000);
        chain.donate(13, 14, 2_000);
        chain
    }

    fn donate(&mut self, asset: u8, holder: u8, amount: u128) {
        submit(
            &mut self.state,
            token(id(holder)),
            &[token(id(holder)), token(vault(asset))],
            &[&key(holder)],
            &Message::Transfer {
                to: vault(asset),
                amount,
            },
        )
        .expect("fund reserve");
    }

    fn actors(&self) -> Vec<Actor> {
        vec![
            processor(),
            token(vault(11)),
            token(vault(13)),
            token(id(2)),
            token(id(15)),
            token(id(21)),
            token(id(22)),
        ]
    }

    fn data(&self, actor: Actor) -> ActorState {
        self.state
            .get_account_by_id(actor.account_id)
            .data
            .actor_state(actor.program_account_id)
            .clone()
    }

    fn snapshot(&self) -> Vec<ActorState> {
        self.actors()
            .into_iter()
            .map(|actor| self.data(actor))
            .collect()
    }

    fn request(&self) -> Redeem {
        Redeem {
            source: id(2),
            recipient_a: id(21),
            recipient_b: id(22),
            shares: 100,
            min_a: 100,
            min_b: 200,
            candidate: Candidate {
                reserve_a: self.balance(vault(11)),
                reserve_b: self.balance(vault(13)),
                supply: self.supply(),
                output_a: 100_u128
                    .checked_mul(self.balance(vault(11)))
                    .expect("quote multiplication")
                    .checked_div(self.supply())
                    .expect("quote supply"),
                output_b: 100_u128
                    .checked_mul(self.balance(vault(13)))
                    .expect("quote multiplication")
                    .checked_div(self.supply())
                    .expect("quote supply"),
            },
            reject_at_completion: false,
        }
    }

    fn call(&mut self, op: Redeem, signed: bool) -> Result<Vec<TransactionEvent>, LeeError> {
        let mut actors = self.actors();
        actors.push(token(op.source));
        actors.push(token(op.recipient_a));
        actors.push(token(op.recipient_b));
        let user = key(2);
        let signers = if signed { vec![&user] } else { Vec::new() };
        submit(
            &mut self.state,
            processor(),
            &actors,
            &signers,
            &Command::Redeem(op),
        )
    }

    fn balance(&self, account: AccountId) -> u128 {
        let Some(TokenState::Holding(holding)) = TokenState::decode(&self.data(token(account)))
        else {
            panic!("expected holding")
        };
        holding.balance
    }

    fn supply(&self) -> u128 {
        let Some(TokenState::Definition(definition)) =
            TokenState::decode(&self.data(token(id(15))))
        else {
            panic!("expected definition")
        };
        definition.total_supply
    }
}

#[test]
fn reads_reserves_and_supply_then_burns_shares_and_pays_both_assets() {
    let mut chain = Chain::new();
    chain.call(chain.request(), true).expect("redeem");
    assert_eq!(chain.balance(id(21)), 100);
    assert_eq!(chain.balance(id(22)), 200);
    assert_eq!(chain.balance(vault(11)), 900);
    assert_eq!(chain.balance(vault(13)), 1_800);
    assert_eq!(chain.balance(id(2)), 900);
    assert_eq!(chain.supply(), 900);
    chain
        .call(chain.request(), true)
        .expect("another redemption at the new balances");
    assert_eq!(chain.balance(id(21)), 200);
    assert_eq!(chain.balance(id(22)), 400);
    let state: State = borsh::from_slice(&chain.data(processor())).expect("state decodes");
    assert_eq!(state.phase, Phase::Ready);
    assert_eq!(state.completed, 2);
}

#[test]
fn an_external_reserve_deposit_changes_the_calculated_output() {
    let mut chain = Chain::new();
    chain.donate(11, 12, 500);
    chain
        .call(chain.request(), true)
        .expect("read current reserve");
    assert_eq!(chain.balance(id(21)), 150);
    assert_eq!(chain.balance(id(22)), 200);
}

#[test]
fn an_external_share_burn_changes_the_denominator() {
    let mut chain = Chain::new();
    submit(
        &mut chain.state,
        token(id(2)),
        &[token(id(2)), token(id(15))],
        &[&key(2)],
        &Message::Burn { amount: 100 },
    )
    .expect("burn outside the redemption protocol");
    chain
        .call(chain.request(), true)
        .expect("read current supply");
    assert_eq!(chain.balance(id(21)), 111);
    assert_eq!(chain.balance(id(22)), 222);
    assert_eq!(chain.balance(id(2)), 800);
    assert_eq!(chain.supply(), 800);
}

#[test]
fn minimum_output_rejects_the_quote_before_settlement() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    let mut op = chain.request();
    op.min_a = 101;
    assert_refused(&chain.call(op, true), "redemption below requested minimum");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn late_rejection_restores_shares_supply_and_both_reserves() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    let mut op = chain.request();
    op.reject_at_completion = true;
    assert_refused(&chain.call(op, true), "example rejects after redemption");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn authenticated_balance_reads_do_not_authorize_share_burning() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    assert_refused(
        &chain.call(chain.request(), false),
        "holding must authorize the burn",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn wrong_assets_and_insufficient_shares_fail_without_effects() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    let mut op = chain.request();
    op.shares = 1_001;
    assert_refused(&chain.call(op, true), "insufficient shares");
    let mut op = chain.request();
    op.source = id(12);
    assert_refused(&chain.call(op, true), "unexpected token definition");
    let mut op = chain.request();
    op.recipient_a = id(22);
    assert_refused(&chain.call(op, true), "unexpected token definition");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn final_holder_can_redeem_all_reserves() {
    let mut chain = Chain::new();
    let mut op = chain.request();
    op.shares = 1_000;
    op.candidate.output_a = 1_000;
    op.candidate.output_b = 2_000;
    op.min_a = 1_000;
    op.min_b = 2_000;
    chain.call(op, true).expect("full redemption");
    assert_eq!(chain.balance(vault(11)), 0);
    assert_eq!(chain.balance(vault(13)), 0);
    assert_eq!(chain.balance(id(21)), 1_000);
    assert_eq!(chain.balance(id(22)), 2_000);
    assert_eq!(chain.balance(id(2)), 0);
    assert_eq!(chain.supply(), 0);
}

#[test]
fn donation_after_quote_rejects_until_requoted() {
    let mut chain = Chain::new();
    let old = chain.request();
    chain.donate(11, 12, 500);
    let before = chain.snapshot();
    assert_refused(&chain.call(old, true), "stale redemption candidate");
    assert_eq!(chain.snapshot(), before);
    chain.call(chain.request(), true).expect("refreshed quote");
    assert_eq!(chain.balance(id(21)), 150);
}

#[test]
fn external_burn_after_quote_rejects_without_effects() {
    let mut chain = Chain::new();
    let old = chain.request();
    submit(
        &mut chain.state,
        token(id(2)),
        &[token(id(2)), token(id(15))],
        &[&key(2)],
        &Message::Burn { amount: 100 },
    )
    .expect("outside burn");
    let before = chain.snapshot();
    assert_refused(&chain.call(old, true), "stale redemption candidate");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn candidate_cannot_increase_redemption_output() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    let mut op = chain.request();
    op.candidate.output_a = 101;
    assert_refused(&chain.call(op, true), "incorrect candidate output");
    assert_eq!(chain.snapshot(), before);
}
