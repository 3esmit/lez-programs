//! Payment workflow with authenticated reads. Read replies authenticate both
//! account and deployment. Pending preserves the request; Settling rejects new operations.
//! Final balance reads also require the emitted credits to have executed immediately.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use token_core::Message as TokenMessage;

use crate::{
    coordinator::{self, Action, Control, Machine, Step},
    ledger::Ledger,
};

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Initialize {
        token_program: AccountId,
        definition: AccountId,
        beneficiary: AccountId,
    },
    Process(Operation),
    Withdraw {
        ledger: AccountId,
        asset: AccountId,
        amount: u128,
        to: AccountId,
    },
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Operation {
    pub source: AccountId,
    pub amount: u128,
    pub pay: u128,
    pub burn: u128,
    pub withdraw_outputs: bool,
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
    pub ledger: Ledger,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum State {
    Ready(Data),
    Running {
        data: Data,
        operation: Option<Operation>,
        machine: Box<Machine>,
    },
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
fn write(state: &State) -> Response {
    Response::set_state(borsh::to_vec(state).expect("state encodes"))
}

fn recipe(input: &ReceiveInput, data: &Data, op: &Operation) -> Machine {
    let refund = op
        .amount
        .checked_sub(op.pay)
        .and_then(|left| left.checked_sub(op.burn))
        .expect("distribution exceeds received amount");
    let custody = vault(input.receiver);
    assert_ne!(op.source, custody, "source must not be the protocol vault");
    let mut registers = BTreeMap::new();
    for account in [op.source, custody, data.beneficiary] {
        let next = u8::try_from(registers.len()).expect("three holdings");
        registers.entry(account).or_insert(next);
    }
    let register = |account| *registers.get(&account).expect("holding");
    let actor = |account| Actor::new(account, data.token_program);
    let mut steps = Vec::new();
    for (&account, &register) in &registers {
        steps.push(Step::Read {
            actor: actor(account),
            definition: data.definition,
            supply: false,
            register,
        });
    }
    for (register, value) in [(3, op.amount), (4, op.pay), (5, op.burn), (6, refund)] {
        steps.push(Step::Constant { register, value });
    }
    // No claim is issued before this specific deposit has reached custody.
    steps.extend([
        Step::Transfer {
            from: actor(op.source),
            to: custody,
            amount: 3,
            seed: None,
        },
        Step::Subtract {
            left: register(op.source),
            right: 3,
            out: register(op.source),
        },
        Step::Add {
            left: register(custody),
            right: 3,
            out: register(custody),
        },
        Step::Read {
            actor: actor(custody),
            definition: data.definition,
            supply: false,
            register: 20,
        },
        Step::Equal {
            left: register(custody),
            right: 20,
        },
        Step::CreditClaim {
            owner: op.source,
            asset: data.definition,
            amount: 3,
        },
        Step::MoveClaim {
            from: op.source,
            to: data.beneficiary,
            asset: data.definition,
            amount: 4,
        },
        Step::DebitClaim {
            owner: op.source,
            asset: data.definition,
            amount: 5,
        },
        Step::Burn {
            from: actor(custody),
            amount: 5,
            seed: Some(vault_seed(input.receiver.account_id)),
        },
        Step::Subtract {
            left: register(custody),
            right: 5,
            out: register(custody),
        },
    ]);
    if op.withdraw_outputs {
        for (owner, amount) in [(data.beneficiary, 4), (op.source, 6)] {
            steps.extend([
                Step::DebitClaim {
                    owner,
                    asset: data.definition,
                    amount,
                },
                Step::Transfer {
                    from: actor(custody),
                    to: owner,
                    amount,
                    seed: Some(vault_seed(input.receiver.account_id)),
                },
                Step::Subtract {
                    left: register(custody),
                    right: amount,
                    out: register(custody),
                },
                Step::Add {
                    left: register(owner),
                    right: amount,
                    out: register(owner),
                },
            ]);
        }
    }
    for (&account, &register) in &registers {
        steps.push(Step::Read {
            actor: actor(account),
            definition: data.definition,
            supply: false,
            register: 20,
        });
        steps.push(Step::Equal {
            left: register,
            right: 20,
        });
    }
    let mut machine = Machine::new(steps);
    machine.ledger = data.ledger.clone();
    machine
}

fn advance(
    input: &ReceiveInput,
    mut data: Data,
    operation: Option<Operation>,
    mut machine: Machine,
) -> Response {
    let action = machine.advance();
    if matches!(action, Action::Finished) {
        if let Some(op) = operation {
            assert!(
                !op.reject_at_completion,
                "example rejects after token effects"
            );
            data.completed = data.completed.checked_add(1).expect("completion overflow");
            data.last_received = op.amount;
        }
        data.ledger = machine.ledger;
        return write(&State::Ready(data));
    }
    let response = write(&State::Running {
        data,
        operation,
        machine: Box::new(machine),
    });
    match action {
        Action::Read(call) => response.send(call),
        Action::Effect(call) => response
            .send(call)
            .send_sync(input.receiver, &Control::Resume),
        Action::Finished => unreachable!("finished above"),
    }
}

fn forward_withdrawal(
    input: &ReceiveInput,
    ledger: AccountId,
    asset: AccountId,
    amount: u128,
    to: AccountId,
) -> Response {
    assert!(input.is_authorized, "claim owner must authorize withdrawal");
    Response::keep_state().send_sync(
        Actor::new(ledger, input.receiver.program_account_id),
        &Control::Withdraw { asset, amount, to },
    )
}

pub fn handle_message(input: &ReceiveInput) -> Response {
    if input.pre_state.is_empty() {
        return match borsh::from_slice(&input.message).expect("command decodes") {
            Command::Withdraw {
                ledger,
                asset,
                amount,
                to,
            } => forward_withdrawal(input, ledger, asset, amount, to),
            Command::Initialize {
                token_program,
                definition,
                beneficiary,
            } => {
                assert!(
                    input.is_authorized,
                    "processor owner must authorize initialization"
                );
                let data = Data {
                    token_program,
                    definition,
                    beneficiary,
                    completed: 0,
                    last_received: 0,
                    ledger: Ledger::default(),
                };
                write(&State::Ready(data)).send(
                    SyncMessage::new(
                        Actor::new(vault(input.receiver), token_program),
                        &TokenMessage::InitializeHolding { definition },
                    )
                    .with_pda_seeds([vault_seed(input.receiver.account_id)]),
                )
            }
            Command::Process(_) => panic!("initialize first"),
        };
    }
    let state: State = borsh::from_slice(&input.pre_state).expect("state decodes");
    if input.from_own_program() {
        return match borsh::from_slice(&input.message).expect("control decodes") {
            Control::AssertReady => {
                assert_eq!(input.from, Some(input.receiver), "guard must be self-sent");
                assert!(
                    matches!(state, State::Ready(_)),
                    "recipe must finish immediately"
                );
                Response::keep_state()
            }
            Control::Resume => {
                assert_eq!(input.from, Some(input.receiver), "resume must be self-sent");
                let State::Running {
                    data,
                    operation,
                    mut machine,
                } = state
                else {
                    panic!("no recipe")
                };
                machine.resume_effect();
                advance(input, data, operation, *machine)
            }
            Control::Withdraw { asset, amount, to } => {
                let State::Ready(data) = state else {
                    panic!("ledger is busy")
                };
                assert_eq!(asset, data.definition, "unknown ledger asset");
                let owner = input.from.expect("own program sender").account_id;
                let machine = coordinator::withdrawal(
                    Actor::new(vault(input.receiver), data.token_program),
                    asset,
                    owner,
                    to,
                    amount,
                    vault_seed(input.receiver.account_id),
                    data.ledger.clone(),
                );
                advance(input, data, None, machine).send_sync(input.receiver, &Control::AssertReady)
            }
        };
    }
    match state {
        State::Running {
            data,
            operation,
            mut machine,
        } => {
            machine.accept_reply(input);
            advance(input, data, operation, *machine)
        }
        State::Ready(data) => match borsh::from_slice(&input.message).expect("command decodes") {
            Command::Initialize { .. } => panic!("already initialized"),
            Command::Withdraw {
                ledger,
                asset,
                amount,
                to,
            } => forward_withdrawal(input, ledger, asset, amount, to),
            Command::Process(operation) => {
                let machine = recipe(input, &data, &operation);
                advance(input, data, Some(operation), machine)
                    .send_sync(input.receiver, &Control::AssertReady)
            }
        },
    }
}
