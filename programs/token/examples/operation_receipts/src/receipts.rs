//! A funded intent has a unique escrow, immutable terms, and single consumption.
//! Funding is a separate transaction. Execution failure preserves the funded receipt.
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
    pub kind: Kind,
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Kind {
    Payment {
        definition: AccountId,
        beneficiary: AccountId,
    },
    Redemption {
        asset_a: AccountId,
        asset_b: AccountId,
        shares: AccountId,
    },
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Payment {
    pub source: AccountId,
    pub amount: u128,
    pub pay: u128,
    pub burn: u128,
    pub reject_at_completion: bool,
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Redeem {
    pub source: AccountId,
    pub shares: u128,
    pub recipient_a: AccountId,
    pub recipient_b: AccountId,
    pub min_a: u128,
    pub min_b: u128,
    pub reject_at_completion: bool,
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Request {
    Payment(Payment),
    Redeem(Redeem),
}
impl Request {
    pub fn source(&self) -> AccountId {
        match self {
            Self::Payment(op) => op.source,
            Self::Redeem(op) => op.source,
        }
    }

    fn amount(&self) -> u128 {
        match self {
            Self::Payment(op) => op.amount,
            Self::Redeem(op) => op.shares,
        }
    }

    fn rejects(&self) -> bool {
        match self {
            Self::Payment(op) => op.reject_at_completion,
            Self::Redeem(op) => op.reject_at_completion,
        }
    }
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Initialize(Config),
    Fund {
        id: u64,
        expires_before: u64,
        request: Request,
    },
    Execute {
        id: u64,
    },
    /// Anyone may return an expired receipt to its fixed owner.
    Expire {
        id: u64,
    },
    /// Send to the owner's actor in this program, which authenticates and forwards.
    Cancel {
        processor: AccountId,
        id: u64,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Phase {
    Ready,
    Funding,
    Funded,
    Executing,
    Cancelling,
}
pub const MAX_RECEIPT_LIFETIME_MILLIS: u64 = 60_000;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Receipt {
    pub id: u64,
    pub expires_before: u64,
    pub request: Request,
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct State {
    pub config: Config,
    pub phase: Phase,
    pub next_id: u64,
    pub completed: u64,
    pub receipt: Option<Receipt>,
    pub machine: Option<Machine>,
}

pub fn escrow_seed(processor: AccountId, id: u64) -> PdaSeed {
    let mut hash = Sha256::new();
    hash.update(b"example-receipt-escrow-v1");
    hash.update(processor.to_bytes());
    hash.update(id.to_le_bytes());
    PdaSeed::new(hash.finalize().into())
}
pub fn escrow(processor: Actor, id: u64) -> AccountId {
    AccountId::for_public_pda(
        &processor.program_account_id,
        &escrow_seed(processor.account_id, id),
    )
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
    Response::set_state(borsh::to_vec(state).expect("receipt state encodes"))
}
fn asset(config: &Config) -> AccountId {
    match config.kind {
        Kind::Payment { definition, .. } => definition,
        Kind::Redemption { shares, .. } => shares,
    }
}
fn read(actor: Actor, definition: AccountId, supply: bool, register: u8) -> Step {
    Step::Read {
        actor,
        definition,
        supply,
        register,
    }
}

fn funding(input: &ReceiveInput, state: &State, receipt: &Receipt) -> Machine {
    let config = &state.config;
    let source = receipt.request.source();
    let amount = receipt.request.amount();
    match (&config.kind, &receipt.request) {
        (Kind::Payment { beneficiary, .. }, Request::Payment(op)) => {
            op.amount
                .checked_sub(op.pay)
                .and_then(|left| left.checked_sub(op.burn))
                .expect("distribution exceeds received amount");
            assert_ne!(
                source, *beneficiary,
                "receipt payment requires distinct payer and beneficiary"
            );
        }
        (
            Kind::Redemption {
                asset_a, asset_b, ..
            },
            Request::Redeem(op),
        ) => {
            assert_ne!(op.shares, 0, "zero redemption");
            assert!(
                op.recipient_a != vault(input.receiver, *asset_a)
                    && op.recipient_b != vault(input.receiver, *asset_b),
                "recipient cannot be its reserve"
            );
        }
        _ => panic!("request does not match receipt configuration"),
    }
    let custody = escrow(input.receiver, receipt.id);
    assert_ne!(source, custody, "source must differ from escrow");
    let actor = |account| Actor::new(account, config.token_program);
    Machine::new(vec![
        Step::Initialize {
            actor: actor(custody),
            definition: asset(config),
            seed: escrow_seed(input.receiver.account_id, receipt.id),
        },
        read(actor(source), asset(config), false, 0),
        Step::Constant {
            register: 1,
            value: amount,
        },
        Step::Transfer {
            from: actor(source),
            to: custody,
            amount: 1,
            seed: None,
        },
        read(actor(custody), asset(config), false, 2),
        Step::Equal { left: 1, right: 2 },
    ])
}

fn payment(
    input: &ReceiveInput,
    state: &State,
    receipt: &Receipt,
    op: &Payment,
    definition: AccountId,
    beneficiary: AccountId,
) -> Machine {
    let actor = |account| Actor::new(account, state.config.token_program);
    let custody = escrow(input.receiver, receipt.id);
    let seed = escrow_seed(input.receiver.account_id, receipt.id);
    let refund = op
        .amount
        .checked_sub(op.pay)
        .and_then(|left| left.checked_sub(op.burn))
        .expect("distribution exceeds received amount");
    Machine::new(vec![
        read(actor(custody), definition, false, 0),
        read(actor(op.source), definition, false, 1),
        read(actor(beneficiary), definition, false, 2),
        Step::Constant {
            register: 3,
            value: op.amount,
        },
        Step::Constant {
            register: 4,
            value: op.pay,
        },
        Step::Constant {
            register: 5,
            value: op.burn,
        },
        Step::Constant {
            register: 6,
            value: refund,
        },
        Step::Equal { left: 0, right: 3 },
        Step::Transfer {
            from: actor(custody),
            to: beneficiary,
            amount: 4,
            seed: Some(seed),
        },
        Step::Subtract {
            left: 0,
            right: 4,
            out: 0,
        },
        Step::Add {
            left: 2,
            right: 4,
            out: 2,
        },
        Step::Burn {
            from: actor(custody),
            amount: 5,
            seed: Some(seed),
        },
        Step::Subtract {
            left: 0,
            right: 5,
            out: 0,
        },
        Step::Transfer {
            from: actor(custody),
            to: op.source,
            amount: 6,
            seed: Some(seed),
        },
        Step::Subtract {
            left: 0,
            right: 6,
            out: 0,
        },
        Step::Add {
            left: 1,
            right: 6,
            out: 1,
        },
        read(actor(custody), definition, false, 20),
        Step::Equal { left: 0, right: 20 },
        read(actor(op.source), definition, false, 20),
        Step::Equal { left: 1, right: 20 },
        read(actor(beneficiary), definition, false, 20),
        Step::Equal { left: 2, right: 20 },
    ])
}

fn redemption(
    input: &ReceiveInput,
    state: &State,
    receipt: &Receipt,
    op: &Redeem,
    assets: [AccountId; 3],
) -> Machine {
    let [a, b, shares] = assets;
    let actor = |account| Actor::new(account, state.config.token_program);
    let custody = escrow(input.receiver, receipt.id);
    let sources = [
        (custody, shares, false, 0),
        (shares, shares, true, 1),
        (vault(input.receiver, a), a, false, 2),
        (vault(input.receiver, b), b, false, 3),
        (op.recipient_a, a, false, 4),
        (op.recipient_b, b, false, 5),
    ];
    let mut steps = Vec::new();
    for (account, definition, supply, register) in sources {
        steps.push(read(actor(account), definition, supply, register));
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
        Step::Equal { left: 0, right: 6 },
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
            from: actor(custody),
            amount: 6,
            seed: Some(escrow_seed(input.receiver.account_id, receipt.id)),
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
    for (asset, to, amount, reserve_register, recipient_register) in
        [(a, op.recipient_a, 9, 2, 4), (b, op.recipient_b, 10, 3, 5)]
    {
        steps.extend([
            Step::Transfer {
                from: actor(vault(input.receiver, asset)),
                to,
                amount,
                seed: Some(vault_seed(input.receiver.account_id, asset)),
            },
            Step::Subtract {
                left: reserve_register,
                right: amount,
                out: reserve_register,
            },
            Step::Add {
                left: recipient_register,
                right: amount,
                out: recipient_register,
            },
        ]);
    }
    for (account, definition, supply, register) in sources {
        steps.push(read(actor(account), definition, supply, 20));
        steps.push(Step::Equal {
            left: register,
            right: 20,
        });
    }
    Machine::new(steps)
}

fn cancellation(input: &ReceiveInput, state: &State, receipt: &Receipt) -> Machine {
    let definition = asset(&state.config);
    let actor = |account| Actor::new(account, state.config.token_program);
    let custody = escrow(input.receiver, receipt.id);
    let source = receipt.request.source();
    // Any extra donation to this dedicated escrow is returned to its fixed owner.
    Machine::new(vec![
        read(actor(custody), definition, false, 0),
        read(actor(source), definition, false, 1),
        Step::Transfer {
            from: actor(custody),
            to: source,
            amount: 0,
            seed: Some(escrow_seed(input.receiver.account_id, receipt.id)),
        },
        Step::Add {
            left: 1,
            right: 0,
            out: 1,
        },
        Step::Constant {
            register: 2,
            value: 0,
        },
        read(actor(custody), definition, false, 20),
        Step::Equal { left: 2, right: 20 },
        read(actor(source), definition, false, 20),
        Step::Equal { left: 1, right: 20 },
    ])
}

fn advance(input: &ReceiveInput, mut state: State) -> Response {
    let action = state.machine.as_mut().expect("receipt recipe").advance();
    if matches!(action, Action::Finished) {
        match state.phase {
            Phase::Funding => {
                state.phase = Phase::Funded;
                state.next_id = state.next_id.checked_add(1).expect("receipt id overflow");
            }
            Phase::Executing => {
                assert!(
                    !state.receipt.as_ref().expect("receipt").request.rejects(),
                    "example rejects after receipt effects"
                );
                state.completed = state.completed.checked_add(1).expect("completion overflow");
                state.phase = Phase::Ready;
                state.receipt = None;
            }
            Phase::Cancelling => {
                state.phase = Phase::Ready;
                state.receipt = None;
            }
            Phase::Ready | Phase::Funded => panic!("no receipt recipe should run"),
        }
        state.machine = None;
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
fn forward_cancel(input: &ReceiveInput, processor: AccountId, id: u64) -> Response {
    assert!(
        input.is_authorized,
        "receipt owner must authorize cancellation"
    );
    Response::keep_state().send_sync(
        Actor::new(processor, input.receiver.program_account_id),
        &Control::Cancel { id },
    )
}

pub fn handle_message(input: &ReceiveInput, is_redemption: bool) -> Response {
    if input.pre_state.is_empty() {
        return match borsh::from_slice(&input.message).expect("command decodes") {
            Command::Cancel { processor, id } => forward_cancel(input, processor, id),
            Command::Initialize(config) => {
                assert!(input.is_authorized, "owner must authorize initialization");
                assert_eq!(
                    matches!(config.kind, Kind::Redemption { .. }),
                    is_redemption,
                    "wrong example configuration"
                );
                let mut response = Response::keep_state();
                if let Kind::Redemption {
                    asset_a,
                    asset_b,
                    shares,
                } = config.kind
                {
                    assert!(
                        asset_a != asset_b && asset_a != shares && asset_b != shares,
                        "asset definitions must differ"
                    );
                    for asset in [asset_a, asset_b] {
                        response = response.send(
                            SyncMessage::new(
                                Actor::new(vault(input.receiver, asset), config.token_program),
                                &TokenMessage::InitializeHolding { definition: asset },
                            )
                            .with_pda_seeds([vault_seed(input.receiver.account_id, asset)]),
                        );
                    }
                }
                response.post_state = write(&State {
                    config,
                    phase: Phase::Ready,
                    next_id: 0,
                    completed: 0,
                    receipt: None,
                    machine: None,
                })
                .post_state;
                response
            }
            Command::Fund { .. } | Command::Execute { .. } | Command::Expire { .. } => {
                panic!("initialize first")
            }
        };
    }
    let mut state: State = borsh::from_slice(&input.pre_state).expect("state decodes");
    if input.from_own_program() {
        return match borsh::from_slice(&input.message).expect("control decodes") {
            Control::AssertReady => {
                assert_eq!(input.from, Some(input.receiver), "guard must be self-sent");
                assert_eq!(state.phase, Phase::Ready, "receipt must finish immediately");
                Response::keep_state()
            }
            Control::AssertFunded => {
                assert_eq!(input.from, Some(input.receiver), "guard must be self-sent");
                assert_eq!(state.phase, Phase::Funded, "receipt must fund immediately");
                Response::keep_state()
            }
            Control::Resume => {
                assert_eq!(input.from, Some(input.receiver), "resume must be self-sent");
                state.machine.as_mut().expect("recipe").resume_effect();
                advance(input, state)
            }
            Control::Cancel { id } => {
                assert_eq!(state.phase, Phase::Funded, "no funded receipt");
                let receipt = state.receipt.as_ref().expect("receipt");
                assert_eq!(receipt.id, id, "wrong receipt id");
                assert_eq!(
                    input.from,
                    Some(Actor::new(
                        receipt.request.source(),
                        input.receiver.program_account_id
                    )),
                    "wrong receipt owner"
                );
                state.machine = Some(cancellation(input, &state, receipt));
                state.phase = Phase::Cancelling;
                advance(input, state).send_sync(input.receiver, &Control::AssertReady)
            }
        };
    }
    if let Some(machine) = &mut state.machine {
        machine.accept_reply(input);
        return advance(input, state);
    }
    match borsh::from_slice(&input.message).expect("command decodes") {
        Command::Initialize(_) => panic!("already initialized"),
        Command::Cancel { processor, id } => forward_cancel(input, processor, id),
        Command::Fund {
            id,
            expires_before,
            request,
        } => {
            assert_eq!(state.phase, Phase::Ready, "receipt already funded");
            assert_eq!(id, state.next_id, "receipt id already used or out of order");
            assert_ne!(expires_before, 0, "receipt expiry must be positive");
            let receipt = Receipt {
                id,
                expires_before,
                request,
            };
            state.machine = Some(funding(input, &state, &receipt));
            state.receipt = Some(receipt);
            state.phase = Phase::Funding;
            advance(input, state)
                .send_sync(input.receiver, &Control::AssertFunded)
                .try_timestamp_window(
                    expires_before.saturating_sub(MAX_RECEIPT_LIFETIME_MILLIS)..expires_before,
                )
                .expect("positive receipt lifetime")
        }
        Command::Expire { id } => {
            assert_eq!(state.phase, Phase::Funded, "no funded receipt");
            let receipt = state.receipt.as_ref().expect("receipt");
            assert_eq!(receipt.id, id, "wrong receipt id");
            let expires_before = receipt.expires_before;
            state.machine = Some(cancellation(input, &state, receipt));
            state.phase = Phase::Cancelling;
            advance(input, state)
                .send_sync(input.receiver, &Control::AssertReady)
                .timestamp_window(expires_before..)
        }
        Command::Execute { id } => {
            assert_eq!(state.phase, Phase::Funded, "no funded receipt");
            let receipt = state.receipt.as_ref().expect("receipt");
            assert_eq!(id, receipt.id, "wrong receipt id");
            let expires_before = receipt.expires_before;
            state.machine = Some(match (&state.config.kind, &receipt.request) {
                (
                    Kind::Payment {
                        definition,
                        beneficiary,
                    },
                    Request::Payment(op),
                ) => payment(input, &state, receipt, op, *definition, *beneficiary),
                (
                    Kind::Redemption {
                        asset_a,
                        asset_b,
                        shares,
                    },
                    Request::Redeem(op),
                ) => redemption(input, &state, receipt, op, [*asset_a, *asset_b, *shares]),
                _ => panic!("receipt configuration mismatch"),
            });
            state.phase = Phase::Executing;
            advance(input, state)
                .send_sync(input.receiver, &Control::AssertReady)
                .timestamp_window(..expires_before)
        }
    }
}
