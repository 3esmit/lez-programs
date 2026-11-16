//! Run the real Token and example guests. The example pays, burns and refunds in one call.
use integration_tests::{account, assert_refused, deploy_at, key, submit};
use lee::{error::LeeError, V03State};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::TransactionEvent,
};
use token_callback_example::receiver::{self, Command, Distribution, State};
use token_core::{
    event::{Burned, TokenEvent, Transferred},
    Message, TokenState, TokensReceived,
};

const TOKEN: AccountId = AccountId::new([0xa0; 32]);
const CALLBACK: AccountId = AccountId::new([0xb0; 32]);
const ATTACK: AccountId = AccountId::new([0xc0; 32]);
const PROBE: AccountId = AccountId::new([0xd0; 32]);

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
        let mut state = deploy_at(&[
            (TOKEN, token_methods::TOKEN_ELF),
            (CALLBACK, token_callback_example::TOKEN_CALLBACK_ELF),
            (ATTACK, token_callback_example::TOKEN_CALLBACK_ATTACK_ELF),
            (PROBE, token_callback_example::TOKEN_CALLBACK_PROBE_ELF),
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

    fn request(amount: u128, pay: u128, burn: u128, reject: bool) -> Message {
        Message::TransferAndCallAtomic {
            to: vault(),
            amount,
            callback_account: processor().account_id,
            callback_program: CALLBACK,
            data: borsh::to_vec(&Distribution {
                pay,
                burn,
                reject_at_completion: reject,
            })
            .expect("request encodes"),
        }
    }

    fn call(&mut self, message: &Message) -> Result<Vec<TransactionEvent>, LeeError> {
        let actors = self.actors();
        submit(
            &mut self.state,
            token(payer()),
            &actors,
            &[&key(2)],
            message,
        )
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

    fn processor_state(&self) -> State {
        borsh::from_slice(
            self.state
                .get_account_by_id(processor().account_id)
                .data
                .actor_state(CALLBACK),
        )
        .expect("processor state decodes")
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

#[test]
fn deferred_private_credit_fails_the_completion_guard() {
    use std::collections::BTreeSet;

    use lee::{
        privacy_preserving_transaction::circuit::{execute_and_prove, ProgramCatalog, Simulation},
        program::Program,
        test_utils::{no_seal, TestPrivateKeys},
    };
    use lee_core::{
        execution_state::{PublicExecutionContext, TransactionEntry},
        AuthorizationSecretKey, ProvingInput, RootCall, SenderPresentation,
    };

    let chain = Chain::new();
    let before = chain.snapshots();
    let private = TestPrivateKeys {
        ask: AuthorizationSecretKey([50; 32]),
        d: [50; 32],
        z: [50; 32],
    };
    let public = chain.actors();
    let programs: ProgramCatalog = [(
        TOKEN,
        Program::new(token_methods::TOKEN_ELF.to_vec().into()).expect("token guest loads"),
    )]
    .into_iter()
    .collect();
    // A foreign private recipient does not participate. Even before mailbox publication
    // validation, the final source self-call must refuse the missing acknowledgement.
    let result = execute_and_prove(
        ProvingInput {
            root: TransactionEntry::Sync(RootCall {
                to: token(payer()),
                message: borsh::to_vec(&Message::TransferAndCallAtomic {
                    to: private.account_id(),
                    amount: 100,
                    callback_account: processor().account_id,
                    callback_program: CALLBACK,
                    data: Vec::new(),
                })
                .expect("command encodes"),
            }),
            public_context: PublicExecutionContext::new(public.clone(), [payer()]),
            private_witnesses: Vec::new(),
            dummy_inputs: Vec::new(),
            ciphertext_padding: None,
            recoveries: Vec::new(),
            private_promotions: BTreeSet::new(),
        },
        &Simulation {
            public_actor_states: public
                .into_iter()
                .map(|actor| {
                    (
                        actor,
                        chain
                            .state
                            .get_account_by_id(actor.account_id)
                            .data
                            .actor_state(actor.program_account_id)
                            .clone(),
                    )
                })
                .collect(),
            admitted_accounts: None,
        },
        &programs,
        |_| SenderPresentation::Canonical,
        |_, _| false,
        no_seal,
    );
    assert_refused(
        &result,
        "atomic credit and callback must complete immediately",
    );
    assert_eq!(chain.snapshots(), before);
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
    assert!(!chain.processor_state().busy);
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
fn callback_cannot_reenter_the_payer_even_with_its_transaction_credential() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let callback = Actor::new(processor().account_id, ATTACK);
    let mut declared = chain.actors();
    declared.push(callback);
    let result = submit(
        &mut chain.state,
        token(payer()),
        &declared,
        &[&key(2)],
        &Message::TransferAndCallAtomic {
            to: vault(),
            amount: 100,
            callback_account: callback.account_id,
            callback_program: callback.program_account_id,
            data: Vec::new(),
        },
    );
    assert_refused(&result, "atomic transfer in progress");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn distribution_cannot_spend_old_vault_funds_to_exceed_its_receipt() {
    let mut chain = Chain::new();
    chain
        .call(&Message::Transfer {
            to: vault(),
            amount: 200,
        })
        .expect("prefund vault");
    let before = chain.snapshots();
    let result = chain.call(&Chain::request(100, 110, 0, false));
    assert_refused(&result, "distribution exceeds received amount");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn missing_callback_participant_reverts_the_credit() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let declared: Vec<_> = chain
        .actors()
        .into_iter()
        .filter(|actor| *actor != processor())
        .collect();
    let result = submit(
        &mut chain.state,
        token(payer()),
        &declared,
        &[&key(2)],
        &Chain::request(100, 60, 10, false),
    );
    assert!(result.is_err(), "undeclared callback cannot complete");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn callback_cannot_target_the_tokens_trusted_internal_namespace() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let result = chain.call(&Message::TransferAndCallAtomic {
        to: vault(),
        amount: 100,
        callback_account: beneficiary(),
        callback_program: TOKEN,
        data: Vec::new(),
    });
    assert_refused(&result, "callback must use another program");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn unsigned_transfer_and_forged_receipt_are_rejected() {
    let mut chain = Chain::new();
    let before = chain.snapshots();
    let declared = chain.actors();
    let result = submit(
        &mut chain.state,
        token(payer()),
        &declared,
        &[],
        &Chain::request(100, 60, 10, false),
    );
    assert_refused(&result, "sender holding must authorize the transfer");
    let result = submit(
        &mut chain.state,
        processor(),
        &declared,
        &[&key(2)],
        &TokensReceived {
            source: payer(),
            destination: vault(),
            definition: definition(),
            amount: 100,
            data: borsh::to_vec(&Distribution {
                pay: 60,
                burn: 10,
                reject_at_completion: false,
            })
            .expect("request encodes"),
        },
    );
    assert_refused(&result, "processor owner must authorize commands");
    assert_eq!(chain.snapshots(), before);
}

#[test]
fn zero_amount_still_executes_and_acknowledges_the_callback() {
    let mut chain = Chain::new();
    chain
        .call(&Chain::request(0, 0, 0, false))
        .expect("zero transfer still calls hook");
    assert_eq!(chain.balance(payer()), 1_000);
    assert_eq!(chain.processor_state().completed, 1);
}

#[test]
fn atomic_credit_does_not_pass_the_sources_pda_grant_to_the_callback() {
    let mut chain = Chain::new();
    chain
        .call(&Message::Transfer {
            to: vault(),
            amount: 100,
        })
        .expect("fund PDA source");
    let probe = Actor::new(vault(), PROBE);
    let mut declared = chain.actors();
    declared.push(probe);
    submit(
        &mut chain.state,
        processor(),
        &declared,
        &[&key(4)],
        &Command::SendFromVault {
            to: beneficiary(),
            amount: 25,
            callback_account: probe.account_id,
            callback_program: probe.program_account_id,
        },
    )
    .expect("hook observes no source PDA authorization");
    assert_eq!(chain.balance(vault()), 75);
    assert_eq!(chain.balance(beneficiary()), 25);
}
