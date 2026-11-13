use integration_tests::{account, assert_refused, deploy_at, key, submit};
use lee::{error::LeeError, V03State};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::TransactionEvent,
};
use token_core::{Message, Payment, SpendPermission, TokenState};
use token_permission_example::spender::Command;

const TOKEN: AccountId = AccountId::new([0xa0; 32]);
const SPENDER: AccountId = AccountId::new([0xb0; 32]);
const OTHER: AccountId = AccountId::new([0xc0; 32]);
fn token(id: AccountId) -> Actor {
    Actor::new(id, TOKEN)
}
fn payer() -> AccountId {
    account(&key(2))
}
fn beneficiary() -> AccountId {
    account(&key(3))
}
fn definition() -> AccountId {
    account(&key(1))
}
fn spender() -> Actor {
    Actor::new(account(&key(4)), SPENDER)
}

struct Chain {
    state: V03State,
}
impl Chain {
    fn new() -> Self {
        let mut state = deploy_at(&[
            (TOKEN, token_methods::TOKEN_ELF),
            (SPENDER, token_permission_example::TOKEN_SPENDER_ELF),
            (OTHER, token_permission_example::TOKEN_SPENDER_ELF),
        ]);
        submit(
            &mut state,
            token(definition()),
            &[token(definition()), token(payer())],
            &[&key(1), &key(2)],
            &Message::NewDefinition {
                name: "Permission example".into(),
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
        Self { state }
    }

    fn actors(&self) -> Vec<Actor> {
        vec![
            token(definition()),
            token(payer()),
            token(beneficiary()),
            spender(),
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

    fn owner(&mut self, message: &Message) -> Result<Vec<TransactionEvent>, LeeError> {
        let actors = self.actors();
        submit(
            &mut self.state,
            token(payer()),
            &actors,
            &[&key(2)],
            message,
        )
    }

    fn approve(&mut self, expiry: u64) {
        self.owner(&Message::ApproveSpending {
            permission: SpendPermission {
                spender_account: spender().account_id,
                spender_program: SPENDER,
                destination: beneficiary(),
                remaining: 100,
                burn_remaining: 10,
                next_nonce: 0,
                expires_before: expiry,
            },
        })
        .expect("owner approves in a separate transaction");
    }

    fn call_as(
        &mut self,
        controller: u8,
        program: AccountId,
        command: &Command,
    ) -> Result<Vec<TransactionEvent>, LeeError> {
        let root = Actor::new(account(&key(controller)), program);
        let mut actors = self.actors();
        actors.push(root);
        submit(&mut self.state, root, &actors, &[&key(controller)], command)
    }

    fn spend_command(
        generation: u64,
        nonce: u64,
        pay: u128,
        burn: u128,
        reject_after: bool,
    ) -> Command {
        Command::Spend {
            token_program: TOKEN,
            source: payer(),
            generation,
            nonce,
            pay,
            burn,
            reject_after,
        }
    }

    fn spend(
        &mut self,
        nonce: u64,
        pay: u128,
        burn: u128,
    ) -> Result<Vec<TransactionEvent>, LeeError> {
        self.call_as(4, SPENDER, &Self::spend_command(1, nonce, pay, burn, false))
    }

    fn balance(&self, id: AccountId) -> u128 {
        match TokenState::decode(&self.data(token(id))) {
            Some(TokenState::Holding(holding) | TokenState::PermittedHolding { holding, .. }) => {
                holding.balance
            }
            Some(TokenState::Definition(_)) | None => panic!("expected holding"),
        }
    }

    fn permission(&self) -> SpendPermission {
        let Some(TokenState::PermittedHolding {
            permission: Some(permission),
            ..
        }) = TokenState::decode(&self.data(token(payer())))
        else {
            panic!("expected permission")
        };
        permission
    }

    fn assert_payment(&self) {
        assert_eq!(self.balance(payer()), 930);
        assert_eq!(self.balance(beneficiary()), 60);
        let Some(TokenState::Definition(def)) = TokenState::decode(&self.data(token(definition())))
        else {
            panic!("expected definition")
        };
        assert_eq!(def.total_supply, 990);
    }
}

#[test]
fn typed_batch_pays_burns_and_returns_change_to_the_source() {
    let mut chain = Chain::new();
    chain
        .owner(&Message::Batch {
            payments: vec![
                Payment {
                    to: beneficiary(),
                    amount: 60,
                },
                Payment {
                    to: payer(),
                    amount: 30,
                },
            ],
            burn: 10,
        })
        .expect("batch");
    chain.assert_payment();
}

#[test]
fn failed_batch_credit_rolls_back_debit_and_supply_change() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    // Sending to the definition is admitted, but it cannot receive a holding credit.
    assert_refused(
        &chain.owner(&Message::Batch {
            payments: vec![Payment {
                to: definition(),
                amount: 60,
            }],
            burn: 10,
        }),
        "recipient is a token definition",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn batch_checks_gross_balance_and_checked_total() {
    let mut chain = Chain::new();
    let before = chain.snapshot();
    assert_refused(
        &chain.owner(&Message::Batch {
            payments: vec![Payment {
                to: payer(),
                amount: 1_001,
            }],
            burn: 0,
        }),
        "insufficient balance",
    );
    assert_refused(
        &chain.owner(&Message::Batch {
            payments: vec![Payment {
                to: payer(),
                amount: u128::MAX,
            }],
            burn: 1,
        }),
        "batch amount overflow",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn delegate_spends_cumulative_limits_without_the_owner_credential() {
    let mut chain = Chain::new();
    chain.approve(100);
    chain.spend(0, 60, 10).expect("bounded payment");
    chain.assert_payment();
    assert_eq!(chain.permission().remaining, 30);
    assert_eq!(chain.permission().burn_remaining, 0);
    assert_eq!(chain.permission().next_nonce, 1);
    chain.spend(1, 20, 0).expect("second partial payment");
    let before = chain.snapshot();
    assert_refused(&chain.spend(2, 11, 0), "spending permission exhausted");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn completed_spend_nonce_cannot_be_replayed_with_budget_remaining() {
    let mut chain = Chain::new();
    chain.approve(100);
    chain.spend(0, 1, 0).expect("first spend");
    let before = chain.snapshot();
    assert_refused(&chain.spend(0, 1, 0), "stale spending nonce");
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn exact_spender_account_and_deployment_are_required() {
    let mut chain = Chain::new();
    chain.approve(100);
    let before = chain.snapshot();
    let cmd = Chain::spend_command(1, 0, 1, 0, false);
    assert_refused(
        &chain.call_as(5, SPENDER, &cmd),
        "caller is not the permitted actor",
    );
    assert_refused(
        &chain.call_as(4, OTHER, &cmd),
        "caller is not the permitted actor",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn ordinary_transfer_cannot_bypass_the_allowance_without_owner_authority() {
    let mut chain = Chain::new();
    chain.approve(100);
    let before = chain.snapshot();
    assert_refused(
        &chain.call_as(
            4,
            SPENDER,
            &Command::Bypass {
                token_program: TOKEN,
                source: payer(),
                destination: beneficiary(),
                amount: 200,
            },
        ),
        "sender holding must authorize",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn owner_credential_still_grants_broad_authority_if_included() {
    let mut chain = Chain::new();
    chain.approve(100);
    let actors = chain.actors();
    submit(
        &mut chain.state,
        spender(),
        &actors,
        &[&key(4), &key(2)],
        &Command::Bypass {
            token_program: TOKEN,
            source: payer(),
            destination: beneficiary(),
            amount: 200,
        },
    )
    .expect("explicit owner credential permits ordinary transfer");
    assert_eq!(chain.balance(payer()), 800);
    assert_eq!(
        chain.permission().remaining,
        100,
        "ordinary owner operation does not consume delegation"
    );
}

#[test]
fn revocation_and_reapproval_do_not_revive_old_generations() {
    let mut chain = Chain::new();
    chain.approve(100);
    chain.owner(&Message::RevokeSpending).expect("revoke");
    assert_refused(&chain.spend(0, 1, 0), "no active spending permission");
    chain.approve(100);
    assert_refused(&chain.spend(0, 1, 0), "stale spending permission");
    chain
        .call_as(4, SPENDER, &Chain::spend_command(2, 0, 60, 10, false))
        .expect("new permission");
    chain.assert_payment();
}

#[test]
fn burn_limit_and_expiry_reject_without_consuming_allowance() {
    let mut chain = Chain::new();
    chain.approve(100);
    let before = chain.snapshot();
    assert_refused(&chain.spend(0, 0, 11), "burn permission exhausted");
    assert_eq!(chain.snapshot(), before);
    chain.approve(0); // Harness timestamp is zero; the upper bound is exclusive.
    let expired = chain.snapshot();
    let result = chain.call_as(4, SPENDER, &Chain::spend_command(2, 0, 1, 0, false));
    assert!(result.is_err(), "expired permission must fail: {result:?}");
    assert_eq!(chain.snapshot(), expired);
}

#[test]
fn late_rejection_restores_balance_supply_budget_and_nonce() {
    let mut chain = Chain::new();
    chain.approve(100);
    let before = chain.snapshot();
    assert_refused(
        &chain.call_as(4, SPENDER, &Chain::spend_command(1, 0, 60, 10, true)),
        "example rejects after spending",
    );
    assert_eq!(chain.snapshot(), before);
    chain.spend(0, 60, 10).expect("retry after rollback");
    chain.assert_payment();
}

#[test]
fn owner_operations_and_incoming_credits_preserve_permission_metadata() {
    let mut chain = Chain::new();
    chain.approve(100);
    let permission = chain.permission();
    chain
        .owner(&Message::InitializeHolding {
            definition: definition(),
        })
        .expect("idempotent initialize");
    chain
        .owner(&Message::Transfer {
            to: payer(),
            amount: 30,
        })
        .expect("self transfer");
    chain
        .owner(&Message::Burn { amount: 1 })
        .expect("owner burn");
    chain
        .owner(&Message::Batch {
            payments: vec![Payment {
                to: payer(),
                amount: 5,
            }],
            burn: 1,
        })
        .expect("owner batch");
    assert_eq!(chain.permission(), permission);
    assert_eq!(chain.balance(payer()), 998);
}

#[test]
fn only_the_owner_can_replace_or_revoke_a_permission() {
    let mut chain = Chain::new();
    chain.approve(100);
    let before = chain.snapshot();
    let actors = chain.actors();
    let message = Message::ApproveSpending {
        permission: chain.permission(),
    };
    assert_refused(
        &submit(&mut chain.state, token(payer()), &actors, &[], &message),
        "owner must authorize spending approval",
    );
    assert_refused(
        &submit(
            &mut chain.state,
            token(payer()),
            &actors,
            &[],
            &Message::RevokeSpending,
        ),
        "owner must authorize spending revocation",
    );
    assert_eq!(chain.snapshot(), before);
}

#[test]
fn delegated_budget_does_not_override_the_actual_balance() {
    let mut chain = Chain::new();
    chain.approve(100);
    chain
        .owner(&Message::Burn { amount: 999 })
        .expect("owner spends independently");
    let before = chain.snapshot();
    assert_refused(&chain.spend(0, 2, 0), "insufficient balance");
    assert_eq!(chain.snapshot(), before);
}
