//! Public actor examples using the unchanged Token guest.
use integration_tests::{account, assert_refused, deploy_at, key, submit};
use lee::{error::LeeError, V03State};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::TransactionEvent,
};
use token_composition_example::processor::{self, Command, Config, Phase, PolicyBehavior, State};
use token_core::{Message, TokenState};

const TOKEN: AccountId = AccountId::new([0xa0; 32]);
const PROCESSOR: AccountId = AccountId::new([0xb0; 32]);
const POLICY: AccountId = AccountId::new([0xc0; 32]);

fn token(id: AccountId) -> Actor {
    Actor::new(id, TOKEN)
}
fn processor() -> Actor {
    Actor::new(account(&key(4)), PROCESSOR)
}
fn policy() -> Actor {
    Actor::new(account(&key(5)), POLICY)
}
fn vault() -> AccountId {
    processor::vault(processor())
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

struct Chain {
    state: V03State,
}

impl Chain {
    fn new(supply: u128) -> Self {
        let mut state = deploy_at(&[
            (TOKEN, token_methods::TOKEN_ELF),
            (PROCESSOR, token_composition_example::TOKEN_COMPOSITION_ELF),
            (POLICY, token_composition_example::TOKEN_POLICY_ELF),
        ]);
        submit(
            &mut state,
            token(definition()),
            &[token(definition()), token(payer())],
            &[&key(1), &key(2)],
            &Message::NewDefinition {
                name: "Composition example".to_owned(),
                total_supply: supply,
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
            &Command::Initialize(Box::new(Config {
                token_program: TOKEN,
                definition: definition(),
                payer: payer(),
                beneficiary: beneficiary(),
                policy: policy(),
                budget: 100,
                max_pay: 60,
                max_burn: 10,
                min_refund: 30,
            })),
        )
        .expect("initialize processor");
        submit(&mut state, policy(), &[policy()], &[&key(5)], &())
            .expect("initialize policy actor");
        Self { state }
    }

    fn actors(&self) -> Vec<Actor> {
        vec![
            token(definition()),
            token(payer()),
            token(beneficiary()),
            token(vault()),
            processor(),
            policy(),
        ]
    }

    fn call(
        &mut self,
        command: &Command,
        payer_signed: bool,
    ) -> Result<Vec<TransactionEvent>, LeeError> {
        let actors = self.actors();
        let payer_key = key(2);
        let signers = if payer_signed {
            vec![&payer_key]
        } else {
            Vec::new()
        };
        submit(&mut self.state, processor(), &actors, &signers, command)
    }

    fn fund(&mut self) {
        submit(
            &mut self.state,
            token(payer()),
            &[token(payer()), token(vault())],
            &[&key(2)],
            &Message::Transfer {
                to: vault(),
                amount: 100,
            },
        )
        .expect("fund vault in separate transaction");
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

    fn balance(&self, id: AccountId) -> u128 {
        let Some(TokenState::Holding(holding)) = TokenState::decode(&self.data(token(id))) else {
            panic!("expected holding")
        };
        holding.balance
    }

    fn assert_finished(&self) {
        assert_eq!(self.balance(payer()), 930);
        assert_eq!(self.balance(beneficiary()), 60);
        assert_eq!(self.balance(vault()), 0);
        let Some(TokenState::Definition(definition)) =
            TokenState::decode(&self.data(token(definition())))
        else {
            panic!("expected definition")
        };
        assert_eq!(definition.total_supply, 990);
        let state: State = borsh::from_slice(&self.data(processor())).expect("processor decodes");
        assert_eq!(state.phase, Phase::Spent);
    }
}

#[test]
fn direct_distribution_keeps_change_in_the_payer() {
    let mut chain = Chain::new(1_000);
    chain
        .call(&Command::Direct { reject: false }, true)
        .expect("direct distribution");
    chain.assert_finished();
}

#[test]
fn direct_distribution_only_requires_the_net_debit() {
    let mut chain = Chain::new(70);
    chain
        .call(&Command::Direct { reject: false }, true)
        .expect("net debit is sufficient");
    assert_eq!(chain.balance(payer()), 0);
    assert_eq!(chain.balance(beneficiary()), 60);
    assert_eq!(chain.balance(vault()), 0);
}

#[test]
fn direct_distribution_cannot_spend_without_payer_authorization() {
    let mut chain = Chain::new(1_000);
    let before = chain.snapshot();
    assert_refused(
        &chain.call(&Command::Direct { reject: false }, false),
        "sender holding must authorize",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn late_direct_rejection_rolls_back_payment_and_burn() {
    let mut chain = Chain::new(1_000);
    let before = chain.snapshot();
    assert_refused(
        &chain.call(&Command::Direct { reject: true }, true),
        "example rejects after token effects",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn prefunded_distribution_uses_no_payer_credential_and_cannot_repeat() {
    let mut chain = Chain::new(1_000);
    chain.fund();
    chain
        .call(&Command::Prefunded { reject: false }, false)
        .expect("prefunded distribution");
    chain.assert_finished();
    let before = chain.snapshot();
    assert_refused(
        &chain.call(&Command::Prefunded { reject: false }, false),
        "operation already started or spent",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn failed_prefunded_operation_preserves_the_deposit_and_owner_can_recover() {
    let mut chain = Chain::new(1_000);
    chain.fund();
    let funded = chain.snapshot();
    assert_refused(
        &chain.call(&Command::Prefunded { reject: true }, false),
        "example rejects after token effects",
    );
    assert_eq!(chain.snapshot(), funded);
    assert_eq!(chain.balance(payer()), 900);
    assert_eq!(chain.balance(vault()), 100);
    assert_refused(
        &chain.call(&Command::Recover { amount: 100 }, false),
        "processor owner must authorize recovery",
    );
    let actors = chain.actors();
    submit(
        &mut chain.state,
        processor(),
        &actors,
        &[&key(4)],
        &Command::Recover { amount: 100 },
    )
    .expect("recover");
    assert_eq!(chain.balance(payer()), 1_000);
    assert_eq!(chain.balance(vault()), 0);
}

#[test]
fn cooperative_policy_quote_is_validated_before_settlement() {
    let mut chain = Chain::new(1_000);
    chain.fund();
    chain
        .call(
            &Command::WithPolicy {
                behavior: PolicyBehavior::Quote,
                reject: false,
            },
            false,
        )
        .expect("policy distribution");
    chain.assert_finished();
    assert_eq!(
        &*chain.data(policy()),
        &[1],
        "ordinary policy actor is allowed to write its own state"
    );
}

#[test]
fn excessive_policy_quote_cannot_consume_the_vault() {
    let mut chain = Chain::new(1_000);
    chain.fund();
    let before = chain.snapshot();
    assert_refused(
        &chain.call(
            &Command::WithPolicy {
                behavior: PolicyBehavior::ExcessiveQuote,
                reject: false,
            },
            false,
        ),
        "payment exceeds approved limit",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn external_policy_cannot_spend_the_unsigned_main_wallet() {
    let mut chain = Chain::new(1_000);
    chain.fund();
    let before = chain.snapshot();
    assert_refused(
        &chain.call(
            &Command::WithPolicy {
                behavior: PolicyBehavior::SpendPayer,
                reject: false,
            },
            false,
        ),
        "sender holding must authorize",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn unanswered_policy_cannot_commit_a_pending_operation() {
    let mut chain = Chain::new(1_000);
    chain.fund();
    let before = chain.snapshot();
    assert_refused(
        &chain.call(
            &Command::WithPolicy {
                behavior: PolicyBehavior::NoReply,
                reject: false,
            },
            false,
        ),
        "policy did not complete settlement",
    );
    assert_eq!(chain.snapshot(), before);
}
