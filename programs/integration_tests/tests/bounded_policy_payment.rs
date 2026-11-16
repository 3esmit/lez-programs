//! Run the real Token and workflow guests. The example pays, burns and refunds in one call.
use integration_tests::{account, assert_refused, deploy_at, key, submit};
use lee::{error::LeeError, V03State};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::TransactionEvent,
};
use token_bounded_policy_example::payment::{self as receiver, Command, Data, Operation, State};
use token_core::{
    event::{Burned, TokenEvent, Transferred},
    Message, TokenState,
};

const TOKEN: AccountId = AccountId::new([0xa0; 32]);
const CALLBACK: AccountId = AccountId::new([0xb0; 32]);

struct Chain {
    state: V03State,
}

fn processor() -> Actor {
    Actor::new(account(&key(4)), CALLBACK)
}
fn token(id: AccountId) -> Actor {
    Actor::new(id, TOKEN)
}
fn vault() -> AccountId {
    receiver::vault(processor())
}
fn definition() -> AccountId {
    account(&key(1))
}
fn payer() -> AccountId {
    account(&key(2))
}
fn beneficiary() -> AccountId {
    account(&key(3))
}

impl Chain {
    fn new() -> Self {
        Self::with_policy(token_bounded_policy_example::policy::payment())
    }

    fn with_policy(policy: token_bounded_policy_example::policy::Policy) -> Self {
        let mut state = deploy_at(&[
            (TOKEN, token_methods::TOKEN_ELF),
            (
                CALLBACK,
                token_bounded_policy_example::BOUNDED_POLICY_PAYMENT_ELF,
            ),
        ]);
        submit(
            &mut state,
            token(definition()),
            &[token(definition()), token(payer())],
            &[&key(1), &key(2)],
            &Message::NewDefinition {
                name: "Callback example".to_owned(),
                total_supply: 1_000,
                mint_authority: None,
                holding: payer(),
            },
        )
        .expect("create token");
        submit(
            &mut state,
            token(beneficiary()),
            &[token(beneficiary()), token(definition())],
            &[&key(3)],
            &Message::InitializeHolding {
                definition: definition(),
            },
        )
        .expect("initialize beneficiary");
        submit(
            &mut state,
            processor(),
            &[processor(), token(vault()), token(definition())],
            &[&key(4)],
            &Command::Initialize {
                token_program: TOKEN,
                definition: definition(),
                beneficiary: beneficiary(),
                policy,
            },
        )
        .expect("initialize processor and vault");
        Self { state }
    }

    fn actors(&self) -> Vec<Actor> {
        vec![
            token(payer()),
            token(vault()),
            token(beneficiary()),
            token(definition()),
            processor(),
        ]
    }

    fn request(amount: u128, pay: u128, burn: u128, reject: bool) -> Command {
        Command::Process(Operation {
            source: payer(),
            amount,
            pay,
            burn,
            reject_at_completion: reject,
        })
    }

    fn call(&mut self, message: &Command) -> Result<Vec<TransactionEvent>, LeeError> {
        let actors = self.actors();
        submit(&mut self.state, processor(), &actors, &[&key(2)], message)
    }

    fn balance(&self, id: AccountId) -> u128 {
        match TokenState::decode(self.state.get_account_by_id(id).data.actor_state(TOKEN)) {
            Some(TokenState::Holding(holding)) => holding.balance,
            other => panic!("expected a completed holding: {other:?}"),
        }
    }

    fn supply(&self) -> u128 {
        match TokenState::decode(
            self.state
                .get_account_by_id(definition())
                .data
                .actor_state(TOKEN),
        ) {
            Some(TokenState::Definition(definition)) => definition.total_supply,
            other => panic!("expected definition: {other:?}"),
        }
    }

    fn processor_state(&self) -> Data {
        match borsh::from_slice(
            self.state
                .get_account_by_id(processor().account_id)
                .data
                .actor_state(CALLBACK),
        )
        .expect("workflow state decodes")
        {
            State::Ready(data) => data,
            state @ (State::Pending { .. } | State::Settling { .. }) => {
                panic!("workflow did not return to Ready: {state:?}")
            }
        }
    }

    fn snapshots(&self) -> Vec<ActorState> {
        self.actors()
            .into_iter()
            .map(|actor| {
                self.state
                    .get_account_by_id(actor.account_id)
                    .data
                    .actor_state(actor.program_account_id)
                    .clone()
            })
            .collect()
    }
}

fn events<E: TokenEvent + borsh::BorshDeserialize>(records: &[TransactionEvent]) -> Vec<E> {
    records
        .iter()
        .filter(|record| record.event.selector == token_core::event::selector(E::NAME))
        .map(|record| borsh::from_slice(&record.event.data).expect("event decodes"))
        .collect()
}

#[test]
fn receives_pays_burns_and_refunds_then_accepts_another_operation() {
    let mut chain = Chain::new();
    let records = chain
        .call(&Chain::request(100, 60, 10, false))
        .expect("atomic distribution");
    assert_eq!(
        (
            chain.balance(payer()),
            chain.balance(vault()),
            chain.balance(beneficiary()),
            chain.supply()
        ),
        (930, 0, 60, 990)
    );
    assert_eq!(chain.processor_state().completed, 1);
    assert_eq!(chain.processor_state().last_received, 100);
    assert_eq!(
        events::<Transferred>(&records)
            .iter()
            .map(|event| event.amount)
            .collect::<Vec<_>>(),
        vec![100, 60, 30]
    );
    assert_eq!(
        events::<Burned>(&records)
            .iter()
            .map(|event| event.amount)
            .collect::<Vec<_>>(),
        vec![10]
    );
    chain
        .call(&Chain::request(20, 5, 2, false))
        .expect("another distribution");
    assert_eq!(
        (
            chain.balance(payer()),
            chain.balance(beneficiary()),
            chain.supply()
        ),
        (923, 65, 988)
    );
    assert_eq!(chain.processor_state().completed, 2);
}

#[test]
fn rejection_after_transfers_and_burn_rolls_back_every_actor() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let result = chain.call(&Chain::request(100, 60, 10, true));
    assert_refused(&result, "example rejects after token effects");
    assert_eq!(chain.snapshots(), before);
    chain
        .call(&Chain::request(100, 60, 10, false))
        .expect("failure left no busy state");
}

#[test]
fn distribution_cannot_spend_old_vault_funds_to_exceed_its_budget() {
    let mut chain = Chain::new();
    let actors = chain.actors();
    submit(
        &mut chain.state,
        token(payer()),
        &actors,
        &[&key(2)],
        &Message::Transfer {
            to: vault(),
            amount: 200,
        },
    )
    .expect("prefund vault");
    let before = chain.snapshots();
    let result = chain.call(&Chain::request(100, 110, 0, false));
    assert_refused(&result, "distribution exceeds received amount");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn snapshots_do_not_authorize_an_unsigned_source() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let actors = chain.actors();
    let result = submit(
        &mut chain.state,
        processor(),
        &actors,
        &[],
        &Chain::request(100, 60, 10, false),
    );
    assert_refused(&result, "sender holding must authorize the transfer");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn missing_beneficiary_is_rejected_during_authenticated_reads() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let declared: Vec<_> = chain
        .actors()
        .into_iter()
        .filter(|actor| *actor != token(beneficiary()))
        .collect();
    let result = submit(
        &mut chain.state,
        processor(),
        &declared,
        &[&key(2)],
        &Chain::request(100, 60, 10, false),
    );
    assert!(result.is_err(), "all required holdings must participate");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn failure_at_burn_reverts_the_preceding_transfers() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let declared: Vec<_> = chain
        .actors()
        .into_iter()
        .filter(|actor| *actor != token(definition()))
        .collect();
    let result = submit(
        &mut chain.state,
        processor(),
        &declared,
        &[&key(2)],
        &Chain::request(100, 60, 10, false),
    );
    assert!(result.is_err(), "burn requires the definition");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn source_must_be_a_holding_with_sufficient_balance() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let result = chain.call(&Chain::request(1_001, 60, 10, false));
    assert_refused(&result, "insufficient balance");
    let result = chain.call(&Command::Process(Operation {
        source: definition(),
        amount: 100,
        pay: 60,
        burn: 10,
        reject_at_completion: false,
    }));
    assert_refused(&result, "expected initialized token holding");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn zero_amount_completes_without_leaving_pending_state() {
    let mut chain = Chain::new();
    chain
        .call(&Chain::request(0, 0, 0, false))
        .expect("zero distribution");
    assert_eq!(chain.balance(payer()), 1_000);
    assert_eq!(chain.processor_state().completed, 1);
}

fn constant_policy(
    pay: u128,
    burn: u128,
    refund: u128,
) -> token_bounded_policy_example::policy::Policy {
    use token_bounded_policy_example::policy::{Op, Policy};
    Policy {
        code: vec![
            Op::Constant(pay),
            Op::Output,
            Op::Constant(burn),
            Op::Output,
            Op::Constant(refund),
            Op::Output,
        ],
    }
}

#[test]
fn approved_module_changes_calculation_without_changing_token() {
    let mut chain = Chain::with_policy(constant_policy(20, 10, 70));
    chain
        .call(&Chain::request(100, 60, 10, false))
        .expect("module payment");
    assert_eq!(
        (
            chain.balance(payer()),
            chain.balance(beneficiary()),
            chain.supply()
        ),
        (970, 20, 990)
    );
}

#[test]
fn module_cannot_exceed_the_users_payment_budget() {
    let mut chain = Chain::with_policy(constant_policy(61, 10, 29));
    let before = chain.snapshots();
    assert_refused(
        &chain.call(&Chain::request(100, 60, 10, false)),
        "policy exceeds authorized payment limits",
    );
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn module_cannot_create_value() {
    let mut chain = Chain::with_policy(constant_policy(60, 10, 31));
    let before = chain.snapshots();
    assert_refused(
        &chain.call(&Chain::request(100, 60, 10, false)),
        "policy does not conserve payment",
    );
    assert_eq!(chain.snapshots(), before);
}
