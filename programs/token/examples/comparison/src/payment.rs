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

use crate::coordinator::{Action, Control, Machine, Step};

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
pub enum State {
    Ready(Data),
    Running {
        data: Data,
        operation: Operation,
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
    Response::set_state(borsh::to_vec(state).expect("payment state encodes"))
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
    let register = |account| *registers.get(&account).expect("participating holding");
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
    for (from, to, amount, seed) in [
        (op.source, custody, 3, None),
        (
            custody,
            data.beneficiary,
            4,
            Some(vault_seed(input.receiver.account_id)),
        ),
    ] {
        steps.push(Step::Transfer {
            from: actor(from),
            to,
            amount,
            seed,
        });
        steps.push(Step::Subtract {
            left: register(from),
            right: amount,
            out: register(from),
        });
        steps.push(Step::Add {
            left: register(to),
            right: amount,
            out: register(to),
        });
    }
    steps.push(Step::Burn {
        from: actor(custody),
        amount: 5,
        seed: Some(vault_seed(input.receiver.account_id)),
    });
    steps.push(Step::Subtract {
        left: register(custody),
        right: 5,
        out: register(custody),
    });
    steps.push(Step::Transfer {
        from: actor(custody),
        to: op.source,
        amount: 6,
        seed: Some(vault_seed(input.receiver.account_id)),
    });
    steps.push(Step::Subtract {
        left: register(custody),
        right: 6,
        out: register(custody),
    });
    steps.push(Step::Add {
        left: register(op.source),
        right: 6,
        out: register(op.source),
    });
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
    Machine::new(steps)
}

fn advance(
    input: &ReceiveInput,
    mut data: Data,
    operation: Operation,
    mut machine: Machine,
) -> Response {
    let action = machine.advance();
    if matches!(action, Action::Finished) {
        assert!(
            !operation.reject_at_completion,
            "example rejects after token effects"
        );
        data.completed = data.completed.checked_add(1).expect("completion overflow");
        data.last_received = operation.amount;
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
        } = borsh::from_slice(&input.message).expect("command decodes")
        else {
            panic!("initialize first")
        };
        let data = Data {
            token_program,
            definition,
            beneficiary,
            completed: 0,
            last_received: 0,
        };
        let call = SyncMessage::new(
            Actor::new(vault(input.receiver), token_program),
            &TokenMessage::InitializeHolding { definition },
        )
        .with_pda_seeds([vault_seed(input.receiver.account_id)]);
        return write(&State::Ready(data)).send(call);
    }
    let state: State = borsh::from_slice(&input.pre_state).expect("state decodes");
    if input.from == Some(input.receiver) {
        return match borsh::from_slice(&input.message).expect("control decodes") {
            Control::AssertReady => {
                assert!(
                    matches!(state, State::Ready(_)),
                    "recipe must finish immediately"
                );
                Response::keep_state()
            }
            Control::Resume => {
                let State::Running {
                    data,
                    operation,
                    mut machine,
                } = state
                else {
                    panic!("no recipe in progress")
                };
                machine.resume_effect();
                advance(input, data, operation, *machine)
            }
        };
    }
    match state {
        State::Ready(data) => {
            let Command::Process(operation) =
                borsh::from_slice(&input.message).expect("command decodes")
            else {
                panic!("already initialized")
            };
            let machine = recipe(input, &data, &operation);
            advance(input, data, operation, machine)
                .send_sync(input.receiver, &Control::AssertReady)
        }
        State::Running {
            data,
            operation,
            mut machine,
        } => {
            machine.accept_reply(input);
            advance(input, data, operation, *machine)
        }
    }
}
