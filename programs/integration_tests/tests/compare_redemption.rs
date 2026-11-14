use integration_tests::{account, assert_refused, deploy_at, key, submit};
use lee::{error::LeeError, V03State};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::TransactionEvent,
};
use token_comparison_example::redemption::{self, Command, Config, Phase, Redeem, State};
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
            (REDEMPTION, token_comparison_example::COMPARE_REDEMPTION_ELF),
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

    fn request() -> Redeem {
        Redeem {
            source: id(2),
            recipient_a: id(21),
            recipient_b: id(22),
            shares: 100,
            min_a: 100,
            min_b: 200,
            withdraw_outputs: true,
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
    chain.call(Chain::request(), true).expect("redeem");
    assert_eq!(chain.balance(id(21)), 100);
    assert_eq!(chain.balance(id(22)), 200);
    assert_eq!(chain.balance(vault(11)), 900);
    assert_eq!(chain.balance(vault(13)), 1_800);
    assert_eq!(chain.balance(id(2)), 900);
    assert_eq!(chain.supply(), 900);
    chain
        .call(Chain::request(), true)
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
        .call(Chain::request(), true)
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
        .call(Chain::request(), true)
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
    let mut op = Chain::request();
    op.min_a = 101;
    assert_refused(&chain.call(op, true), "redemption below requested minimum");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn late_rejection_restores_shares_supply_and_both_reserves() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    let mut op = Chain::request();
    op.reject_at_completion = true;
    assert_refused(&chain.call(op, true), "example rejects after redemption");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn authenticated_balance_reads_do_not_authorize_share_burning() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    assert_refused(
        &chain.call(Chain::request(), false),
        "holding must authorize the burn",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn wrong_assets_and_insufficient_shares_fail_without_effects() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    let mut op = Chain::request();
    op.shares = 1_001;
    assert_refused(&chain.call(op, true), "insufficient shares");
    let mut op = Chain::request();
    op.source = id(12);
    assert_refused(&chain.call(op, true), "unexpected token definition");
    let mut op = Chain::request();
    op.recipient_a = id(22);
    assert_refused(&chain.call(op, true), "unexpected token definition");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn final_holder_can_redeem_all_reserves() {
    let mut chain = Chain::new();
    let mut op = Chain::request();
    op.shares = 1_000;
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

fn ledger(chain: &Chain) -> token_comparison_example::ledger::Ledger {
    let state: State = borsh::from_slice(&chain.data(processor())).expect("ledger state");
    state.ledger
}
fn withdraw(
    chain: &mut Chain,
    owner: u8,
    asset: u8,
    amount: u128,
    signed: bool,
) -> Result<Vec<TransactionEvent>, LeeError> {
    let router = Actor::new(id(owner), REDEMPTION);
    let mut actors = chain.actors();
    actors.push(router);
    let signer = key(owner);
    let signers = if signed { vec![&signer] } else { Vec::new() };
    submit(
        &mut chain.state,
        router,
        &actors,
        &signers,
        &Command::Withdraw {
            ledger: processor().account_id,
            asset: id(asset),
            amount,
            to: id(owner),
        },
    )
}

#[test]
fn outstanding_claims_are_not_counted_as_reserves_for_the_next_redemption() {
    let mut chain = Chain::new();
    let mut first = Chain::request();
    first.withdraw_outputs = false;
    chain.call(first, true).expect("internal allocation");
    assert_eq!(
        (chain.balance(vault(11)), chain.balance(vault(13))),
        (1_000, 2_000)
    );
    assert_eq!(
        (
            ledger(&chain).claim(id(21), id(11)),
            ledger(&chain).claim(id(22), id(13))
        ),
        (100, 200)
    );
    chain
        .call(Chain::request(), true)
        .expect("second redemption excludes claims");
    assert_eq!((chain.balance(id(21)), chain.balance(id(22))), (100, 200));
    withdraw(&mut chain, 21, 11, 100, true).expect("claim A");
    withdraw(&mut chain, 22, 13, 200, true).expect("claim B");
    assert_eq!((chain.balance(id(21)), chain.balance(id(22))), (200, 400));
    assert_eq!(
        (
            chain.balance(vault(11)),
            chain.balance(vault(13)),
            chain.supply()
        ),
        (800, 1_600, 800)
    );
    assert!(ledger(&chain).claims.is_empty());
}

#[test]
fn caller_cannot_withdraw_someone_elses_claim_or_a_protocol_reserve() {
    let mut chain = Chain::new();
    let mut op = Chain::request();
    op.withdraw_outputs = false;
    chain.call(op, true).expect("internal allocation");
    let before = chain.snapshot();
    assert_refused(
        &withdraw(&mut chain, 21, 11, 100, false),
        "claim owner must authorize withdrawal",
    );
    // A valid signature from another account still provides no claim balance.
    let router = Actor::new(id(4), REDEMPTION);
    let actors = chain.actors();
    assert_refused(
        &submit(
            &mut chain.state,
            router,
            &actors,
            &[&key(4)],
            &Command::Withdraw {
                ledger: processor().account_id,
                asset: id(11),
                amount: 1,
                to: id(21),
            },
        ),
        "insufficient internal claim",
    );
    assert_eq!(chain.snapshot(), before);
}
