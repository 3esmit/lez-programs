//! Public payment workflow in the style of AMM PR 488. Read replies authenticate both
//! account and deployment. Pending preserves the request; Settling rejects new operations.
//! Final balance reads also require the emitted credits to have executed immediately.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, StateReply, SyncMessage},
};
use token_core::{Message as TokenMessage, TokenHolding, TokenState};

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Initialize {
        token_program: AccountId,
        definition: AccountId,
        beneficiary: AccountId,
    },
    Process(Operation),
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Operation {
    pub source: AccountId,
    pub amount: u128,
    pub pay: u128,
    pub burn: u128,
    /// Demonstration switch: fail after the transfers, burn and final balance checks.
    pub reject_at_completion: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Data {
    pub token_program: AccountId,
    pub definition: AccountId,
    pub beneficiary: AccountId,
    pub completed: u64,
    pub last_received: u128,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum ReadStage {
    Source,
    Vault {
        source_balance: u128,
    },
    Beneficiary {
        source_balance: u128,
        vault_balance: u128,
    },
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct BalanceCheck {
    pub account: AccountId,
    pub balance: u128,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum State {
    Ready(Data),
    Pending {
        data: Data,
        operation: Operation,
        stage: ReadStage,
    },
    Settling {
        next: Data,
        remaining: Vec<BalanceCheck>,
        expected: Option<BalanceCheck>,
        checking: bool,
        reject: bool,
    },
}

#[derive(BorshSerialize, BorshDeserialize)]
enum Internal {
    BeginChecks,
    Complete,
    AssertReady,
}

pub fn vault_seed(processor: AccountId) -> PdaSeed {
    PdaSeed::new(processor.to_bytes())
}
pub fn vault(processor: Actor) -> AccountId {
    AccountId::for_public_pda(
        &processor.program_account_id,
        &vault_seed(processor.account_id),
    )
}
fn write(state: State) -> Response {
    Response::set_state(borsh::to_vec(&state).expect("workflow state encodes"))
}
fn read(account: AccountId, program: AccountId) -> SyncMessage {
    SyncMessage::new(Actor::new(account, program), &TokenMessage::ReadState)
}
fn vault_call(input: &ReceiveInput, data: &Data, message: &TokenMessage) -> SyncMessage {
    SyncMessage::new(
        Actor::new(vault(input.receiver), data.token_program),
        message,
    )
    .with_pda_seeds([vault_seed(input.receiver.account_id)])
}
fn snapshot(input: &ReceiveInput, data: &Data, account: AccountId) -> TokenHolding {
    assert_eq!(
        input.from,
        Some(Actor::new(account, data.token_program)),
        "unexpected workflow reply sender"
    );
    let reply: StateReply = borsh::from_slice(&input.message).expect("state reply decodes");
    let Some(TokenState::Holding(holding)) = TokenState::decode(&reply.state) else {
        panic!("expected initialized token holding")
    };
    assert_eq!(
        holding.definition_id, data.definition,
        "unexpected token definition"
    );
    holding
}

pub fn handle_message(input: &ReceiveInput) -> Response {
    if input.pre_state.is_empty() {
        assert!(
            input.is_authorized,
            "processor owner must authorize initialization"
        );
        let Command::Initialize {
            token_program,
            definition,
            beneficiary,
        } = borsh::from_slice(&input.message).expect("initialization decodes")
        else {
            panic!("processor must be initialized")
        };
        let data = Data {
            token_program,
            definition,
            beneficiary,
            completed: 0,
            last_received: 0,
        };
        let call = vault_call(
            input,
            &data,
            &TokenMessage::InitializeHolding { definition },
        );
        return write(State::Settling {
            next: data,
            remaining: Vec::new(),
            expected: None,
            checking: true,
            reject: false,
        })
        .send(call)
        .send_sync(input.receiver, &Internal::Complete);
    }
    let state: State = borsh::from_slice(&input.pre_state).expect("workflow state decodes");
    if input.from == Some(input.receiver) {
        let message: Internal =
            borsh::from_slice(&input.message).expect("internal command decodes");
        return internal(input, state, message);
    }
    match state {
        State::Ready(data) => {
            let Command::Process(operation) =
                borsh::from_slice(&input.message).expect("workflow command decodes")
            else {
                panic!("processor already initialized")
            };
            let call = read(operation.source, data.token_program);
            write(State::Pending {
                data,
                operation,
                stage: ReadStage::Source,
            })
            .send(call)
            .send_sync(input.receiver, &Internal::AssertReady)
        }
        State::Pending {
            data,
            operation,
            stage,
        } => reply(input, data, operation, stage),
        State::Settling {
            next,
            remaining,
            expected,
            checking,
            reject,
        } => {
            let check =
                expected.expect("workflow is settling; only its self-completion is allowed");
            let holding = snapshot(input, &next, check.account);
            assert_eq!(
                holding.balance, check.balance,
                "required token credit did not settle immediately"
            );
            next_check(input, next, remaining, checking, reject)
        }
    }
}

fn internal(input: &ReceiveInput, state: State, message: Internal) -> Response {
    match message {
        Internal::AssertReady => {
            assert!(
                matches!(state, State::Ready(_)),
                "workflow must complete immediately"
            );
            Response::keep_state()
        }
        Internal::BeginChecks => {
            let State::Settling {
                next,
                remaining,
                expected,
                checking,
                reject,
            } = state
            else {
                panic!("no settlement in progress")
            };
            assert!(
                !checking && expected.is_none(),
                "settlement checks already started"
            );
            next_check(input, next, remaining, true, reject)
        }
        Internal::Complete => {
            let State::Settling {
                next,
                remaining,
                expected,
                checking,
                reject,
            } = state
            else {
                panic!("no settlement in progress")
            };
            assert!(
                checking && remaining.is_empty() && expected.is_none(),
                "settlement checks incomplete"
            );
            assert!(!reject, "example rejects after token effects");
            write(State::Ready(next))
        }
    }
}

fn reply(input: &ReceiveInput, data: Data, operation: Operation, stage: ReadStage) -> Response {
    match stage {
        ReadStage::Source => {
            let source = snapshot(input, &data, operation.source);
            let call = read(vault(input.receiver), data.token_program);
            write(State::Pending {
                data,
                operation,
                stage: ReadStage::Vault {
                    source_balance: source.balance,
                },
            })
            .send(call)
        }
        ReadStage::Vault { source_balance } => {
            let holding = snapshot(input, &data, vault(input.receiver));
            let call = read(data.beneficiary, data.token_program);
            write(State::Pending {
                data,
                operation,
                stage: ReadStage::Beneficiary {
                    source_balance,
                    vault_balance: holding.balance,
                },
            })
            .send(call)
        }
        ReadStage::Beneficiary {
            source_balance,
            vault_balance,
        } => {
            let beneficiary = snapshot(input, &data, data.beneficiary);
            settle(
                input,
                data,
                operation,
                source_balance,
                vault_balance,
                beneficiary.balance,
            )
        }
    }
}

fn debit(balances: &mut BTreeMap<AccountId, u128>, account: AccountId, amount: u128) {
    let balance = balances
        .get_mut(&account)
        .expect("participating holding was read");
    *balance = balance.checked_sub(amount).expect("insufficient balance");
}
fn credit(balances: &mut BTreeMap<AccountId, u128>, account: AccountId, amount: u128) {
    let balance = balances
        .get_mut(&account)
        .expect("participating holding was read");
    *balance = balance.checked_add(amount).expect("balance overflow");
}

fn settle(
    input: &ReceiveInput,
    mut data: Data,
    operation: Operation,
    source_balance: u128,
    vault_balance: u128,
    beneficiary_balance: u128,
) -> Response {
    let Operation {
        source,
        amount,
        pay,
        burn,
        reject_at_completion,
    } = operation;
    let refund = amount
        .checked_sub(pay)
        .and_then(|left| left.checked_sub(burn))
        .expect("distribution exceeds received amount");
    let vault_id = vault(input.receiver);
    let mut balances: BTreeMap<_, _> = [
        (source, source_balance),
        (vault_id, vault_balance),
        (data.beneficiary, beneficiary_balance),
    ]
    .into_iter()
    .collect();
    // Compute postconditions in operation order. A map also handles aliases, such as
    // a beneficiary that is the payer, without inventing independent balances.
    debit(&mut balances, source, amount);
    credit(&mut balances, vault_id, amount);
    debit(&mut balances, vault_id, pay);
    credit(&mut balances, data.beneficiary, pay);
    debit(&mut balances, vault_id, burn);
    debit(&mut balances, vault_id, refund);
    credit(&mut balances, source, refund);
    let calls = [
        SyncMessage::new(
            Actor::new(source, data.token_program),
            &TokenMessage::Transfer {
                to: vault_id,
                amount,
            },
        ),
        vault_call(
            input,
            &data,
            &TokenMessage::Transfer {
                to: data.beneficiary,
                amount: pay,
            },
        ),
        vault_call(input, &data, &TokenMessage::Burn { amount: burn }),
        vault_call(
            input,
            &data,
            &TokenMessage::Transfer {
                to: source,
                amount: refund,
            },
        ),
    ];
    data.last_received = amount;
    data.completed = data
        .completed
        .checked_add(1)
        .expect("completion count overflow");
    let mut response = write(State::Settling {
        next: data,
        remaining: balances
            .into_iter()
            .map(|(account, balance)| BalanceCheck { account, balance })
            .collect(),
        expected: None,
        checking: false,
        reject: reject_at_completion,
    });
    for call in calls {
        response = response.send(call);
    }
    response
        .send_sync(input.receiver, &Internal::BeginChecks)
        .send_sync(input.receiver, &Internal::Complete)
}

fn next_check(
    _input: &ReceiveInput,
    next: Data,
    mut remaining: Vec<BalanceCheck>,
    checking: bool,
    reject: bool,
) -> Response {
    let expected = remaining.pop();
    let call = expected
        .as_ref()
        .map(|check| read(check.account, next.token_program));
    let response = write(State::Settling {
        next,
        remaining,
        expected,
        checking,
        reject,
    });
    match call {
        Some(call) => response.send(call),
        None => response,
    }
}
