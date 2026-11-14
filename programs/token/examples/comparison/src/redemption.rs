//! Redeem shares for a proportional amount of two reserves. All reads use Token messages.
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use sha2::{Digest as _, Sha256};
use token_core::Message as TokenMessage;

use crate::{
    coordinator::{self, Action, Control, Failure, Machine, Step},
    ledger::Ledger,
};

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
    pub withdraw_outputs: bool,
    pub reject_at_completion: bool,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Initialize(Config),
    Redeem(Redeem),
    Withdraw {
        ledger: AccountId,
        asset: AccountId,
        amount: u128,
        to: AccountId,
    },
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
    pub ledger: Ledger,
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

fn recipe(input: &ReceiveInput, config: &Config, op: &Redeem, ledger: &Ledger) -> Machine {
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
        Step::SyncReserve {
            asset: config.asset_a,
            actual: 2,
            out: 12,
        },
        Step::SyncReserve {
            asset: config.asset_b,
            actual: 3,
            out: 13,
        },
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
            right: 12,
            divisor: 1,
            out: 9,
        },
        Step::MulDiv {
            left: 6,
            right: 13,
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
        Step::AllocateReserve {
            owner: op.recipient_a,
            asset: config.asset_a,
            amount: 9,
        },
        Step::AllocateReserve {
            owner: op.recipient_b,
            asset: config.asset_b,
            amount: 10,
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
    ]);
    if op.withdraw_outputs {
        for (asset, custody, recipient, amount, custody_register, recipient_register) in [
            (config.asset_a, a, op.recipient_a, 9, 2, 4),
            (config.asset_b, b, op.recipient_b, 10, 3, 5),
        ] {
            steps.extend([
                Step::DebitClaim {
                    owner: recipient,
                    asset,
                    amount,
                },
                Step::Transfer {
                    from: actor(custody),
                    to: recipient,
                    amount,
                    seed: Some(vault_seed(input.receiver.account_id, asset)),
                },
                Step::Subtract {
                    left: custody_register,
                    right: amount,
                    out: custody_register,
                },
                Step::Add {
                    left: recipient_register,
                    right: amount,
                    out: recipient_register,
                },
            ]);
        }
    }
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
    let mut machine = Machine::new(steps);
    machine.ledger = ledger.clone();
    machine
}

fn advance(input: &ReceiveInput, mut state: State) -> Response {
    let action = state.machine.as_mut().expect("recipe").advance();
    if matches!(action, Action::Finished) {
        if let Some(op) = state.operation.take() {
            assert!(!op.reject_at_completion, "example rejects after redemption");
            state.completed = state.completed.checked_add(1).expect("completion overflow");
        }
        state.ledger = state.machine.take().expect("recipe").ledger;
        state.phase = Phase::Ready;
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
            Command::Redeem(_) => panic!("initialize first"),
            Command::Initialize(config) => {
                assert!(input.is_authorized, "owner must authorize initialization");
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
                    ledger: Ledger::default(),
                })
                .post_state;
                response
            }
        };
    }
    let mut state: State = borsh::from_slice(&input.pre_state).expect("state decodes");
    if input.from_own_program() {
        return match borsh::from_slice(&input.message).expect("control decodes") {
            Control::AssertReady => {
                assert_eq!(input.from, Some(input.receiver), "guard must be self-sent");
                assert_eq!(state.phase, Phase::Ready, "recipe must finish immediately");
                Response::keep_state()
            }
            Control::Resume => {
                assert_eq!(input.from, Some(input.receiver), "resume must be self-sent");
                state.machine.as_mut().expect("recipe").resume_effect();
                advance(input, state)
            }
            Control::Withdraw { asset, amount, to } => {
                assert_eq!(state.phase, Phase::Ready, "ledger is busy");
                assert!(
                    asset == state.config.asset_a || asset == state.config.asset_b,
                    "unknown ledger asset"
                );
                let owner = input.from.expect("own program sender").account_id;
                state.machine = Some(coordinator::withdrawal(
                    Actor::new(vault(input.receiver, asset), state.config.token_program),
                    asset,
                    owner,
                    to,
                    amount,
                    vault_seed(input.receiver.account_id, asset),
                    state.ledger.clone(),
                ));
                state.operation = None;
                state.phase = Phase::Running;
                advance(input, state).send_sync(input.receiver, &Control::AssertReady)
            }
        };
    }
    if state.phase == Phase::Running {
        state.machine.as_mut().expect("recipe").accept_reply(input);
        return advance(input, state);
    }
    match borsh::from_slice(&input.message).expect("command decodes") {
        Command::Initialize(_) => panic!("already initialized"),
        Command::Withdraw {
            ledger,
            asset,
            amount,
            to,
        } => forward_withdrawal(input, ledger, asset, amount, to),
        Command::Redeem(operation) => {
            state.machine = Some(recipe(input, &state.config, &operation, &state.ledger));
            state.operation = Some(operation);
            state.phase = Phase::Running;
            advance(input, state).send_sync(input.receiver, &Control::AssertReady)
        }
    }
}
