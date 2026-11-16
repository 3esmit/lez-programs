//! Redeem shares for a proportional amount of two reserves. All reads use Token messages.
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use sha2::{Digest as _, Sha256};
use token_core::Message as TokenMessage;

use crate::coordinator::{Action, Control, Failure, Machine, Step};

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub token_program: AccountId,
    pub asset_a: AccountId,
    pub asset_b: AccountId,
    pub shares: AccountId,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Redeem {
    pub source: AccountId,
    pub recipient_a: AccountId,
    pub recipient_b: AccountId,
    pub shares: u128,
    pub min_a: u128,
    pub min_b: u128,
    pub reject_at_completion: bool,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Initialize(Config),
    Redeem(Redeem),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Phase {
    Ready,
    Running,
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct State {
    pub config: Config,
    pub phase: Phase,
    pub completed: u64,
    pub operation: Option<Redeem>,
    pub machine: Option<Machine>,
}

pub fn vault_seed(processor: AccountId, asset: AccountId) -> PdaSeed {
    let mut hash = Sha256::new();
    hash.update(b"example-redemption-vault-v1");
    hash.update(processor.to_bytes());
    hash.update(asset.to_bytes());
    PdaSeed::new(hash.finalize().into())
}
pub fn vault(processor: Actor, asset: AccountId) -> AccountId {
    AccountId::for_public_pda(
        &processor.program_account_id,
        &vault_seed(processor.account_id, asset),
    )
}
fn write(state: &State) -> Response {
    Response::set_state(borsh::to_vec(state).expect("redemption state encodes"))
}

fn recipe(input: &ReceiveInput, config: &Config, op: &Redeem) -> Machine {
    let actor = |account| Actor::new(account, config.token_program);
    let a = vault(input.receiver, config.asset_a);
    let b = vault(input.receiver, config.asset_b);
    assert_ne!(op.shares, 0, "zero redemption");
    assert!(
        op.recipient_a != a && op.recipient_b != b,
        "recipient cannot be its reserve"
    );
    let sources = [
        (op.source, config.shares, false, 0),
        (config.shares, config.shares, true, 1),
        (a, config.asset_a, false, 2),
        (b, config.asset_b, false, 3),
        (op.recipient_a, config.asset_a, false, 4),
        (op.recipient_b, config.asset_b, false, 5),
    ];
    let mut steps = Vec::new();
    for (account, definition, supply, register) in sources {
        steps.push(Step::Read {
            actor: actor(account),
            definition,
            supply,
            register,
        });
    }
    steps.extend([
        Step::Constant {
            register: 6,
            value: op.shares,
        },
        Step::Constant {
            register: 7,
            value: op.min_a,
        },
        Step::Constant {
            register: 8,
            value: op.min_b,
        },
        Step::AtLeast {
            left: 0,
            right: 6,
            failure: Failure::Shares,
        },
        Step::AtLeast {
            left: 1,
            right: 6,
            failure: Failure::Supply,
        },
        Step::MulDiv {
            left: 6,
            right: 2,
            divisor: 1,
            out: 9,
        },
        Step::MulDiv {
            left: 6,
            right: 3,
            divisor: 1,
            out: 10,
        },
        Step::Positive { register: 9 },
        Step::Positive { register: 10 },
        Step::AtLeast {
            left: 9,
            right: 7,
            failure: Failure::Minimum,
        },
        Step::AtLeast {
            left: 10,
            right: 8,
            failure: Failure::Minimum,
        },
        Step::Burn {
            from: actor(op.source),
            amount: 6,
            seed: None,
        },
        Step::Subtract {
            left: 0,
            right: 6,
            out: 0,
        },
        Step::Subtract {
            left: 1,
            right: 6,
            out: 1,
        },
        Step::Transfer {
            from: actor(a),
            to: op.recipient_a,
            amount: 9,
            seed: Some(vault_seed(input.receiver.account_id, config.asset_a)),
        },
        Step::Subtract {
            left: 2,
            right: 9,
            out: 2,
        },
        Step::Add {
            left: 4,
            right: 9,
            out: 4,
        },
        Step::Transfer {
            from: actor(b),
            to: op.recipient_b,
            amount: 10,
            seed: Some(vault_seed(input.receiver.account_id, config.asset_b)),
        },
        Step::Subtract {
            left: 3,
            right: 10,
            out: 3,
        },
        Step::Add {
            left: 5,
            right: 10,
            out: 5,
        },
    ]);
    for (account, definition, supply, register) in sources {
        steps.push(Step::Read {
            actor: actor(account),
            definition,
            supply,
            register: 20,
        });
        steps.push(Step::Equal {
            left: register,
            right: 20,
        });
    }
    Machine::new(steps)
}

fn advance(input: &ReceiveInput, mut state: State) -> Response {
    let action = state.machine.as_mut().expect("recipe").advance();
    if matches!(action, Action::Finished) {
        assert!(
            !state
                .operation
                .as_ref()
                .expect("operation")
                .reject_at_completion,
            "example rejects after redemption"
        );
        state.phase = Phase::Ready;
        state.machine = None;
        state.operation = None;
        state.completed = state.completed.checked_add(1).expect("completion overflow");
        return write(&state);
    }
    let response = write(&state);
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
        assert!(input.is_authorized, "owner must authorize initialization");
        let Command::Initialize(config) =
            borsh::from_slice(&input.message).expect("command decodes")
        else {
            panic!("initialize first")
        };
        assert!(
            config.asset_a != config.asset_b
                && config.asset_a != config.shares
                && config.asset_b != config.shares,
            "asset definitions must differ"
        );
        let mut response = Response::keep_state();
        for asset in [config.asset_a, config.asset_b] {
            response = response.send(
                SyncMessage::new(
                    Actor::new(vault(input.receiver, asset), config.token_program),
                    &TokenMessage::InitializeHolding { definition: asset },
                )
                .with_pda_seeds([vault_seed(input.receiver.account_id, asset)]),
            );
        }
        response.post_state = write(&State {
            config,
            phase: Phase::Ready,
            completed: 0,
            operation: None,
            machine: None,
        })
        .post_state;
        return response;
    }
    let mut state: State = borsh::from_slice(&input.pre_state).expect("state decodes");
    if input.from == Some(input.receiver) {
        return match borsh::from_slice(&input.message).expect("control decodes") {
            Control::AssertReady => {
                assert_eq!(state.phase, Phase::Ready, "recipe must finish immediately");
                Response::keep_state()
            }
            Control::Resume => {
                state.machine.as_mut().expect("recipe").resume_effect();
                advance(input, state)
            }
        };
    }
    if state.phase == Phase::Running {
        state.machine.as_mut().expect("recipe").accept_reply(input);
        return advance(input, state);
    }
    let Command::Redeem(operation) = borsh::from_slice(&input.message).expect("command decodes")
    else {
        panic!("already initialized")
    };
    state.machine = Some(recipe(input, &state.config, &operation));
    state.operation = Some(operation);
    state.phase = Phase::Running;
    advance(input, state).send_sync(input.receiver, &Control::AssertReady)
}
