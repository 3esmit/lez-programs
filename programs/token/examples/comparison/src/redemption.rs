//! Redeem shares for a proportional amount of two reserves. All reads use Token messages.
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, StateReply, SyncMessage},
};
use sha2::{Digest as _, Sha256};
use token_core::{Message as TokenMessage, TokenState};

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
    pub candidate: Candidate,
    pub reject_at_completion: bool,
}

/// Expected reserves/supply and proposed outputs; these fields carry no authority.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Candidate {
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub supply: u128,
    pub output_a: u128,
    pub output_b: u128,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Initialize(Config),
    Redeem(Redeem),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Phase {
    Ready,
    Reading,
    Settling,
    Checking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub enum Slot {
    Source,
    Supply,
    ReserveA,
    ReserveB,
    RecipientA,
    RecipientB,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Read {
    pub slot: Slot,
    pub account: AccountId,
    pub definition: AccountId,
    pub expected_value: Option<u128>,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct State {
    pub config: Config,
    pub phase: Phase,
    pub completed: u64,
    pub operation: Option<Redeem>,
    pub expected: Option<Read>,
    pub remaining: Vec<Read>,
    pub observed: BTreeMap<Slot, u128>,
}

#[derive(BorshSerialize, BorshDeserialize)]
enum Internal {
    BeginChecks,
    Complete,
    AssertReady,
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
fn token(state: &State, account: AccountId) -> Actor {
    Actor::new(account, state.config.token_program)
}
fn vault_call(
    input: &ReceiveInput,
    state: &State,
    asset: AccountId,
    message: &TokenMessage,
) -> SyncMessage {
    SyncMessage::new(token(state, vault(input.receiver, asset)), message)
        .with_pda_seeds([vault_seed(input.receiver.account_id, asset)])
}
fn reads(input: &ReceiveInput, state: &State, op: &Redeem) -> Vec<Read> {
    vec![
        Read {
            slot: Slot::Source,
            account: op.source,
            definition: state.config.shares,
            expected_value: None,
        },
        Read {
            slot: Slot::Supply,
            account: state.config.shares,
            definition: state.config.shares,
            expected_value: None,
        },
        Read {
            slot: Slot::ReserveA,
            account: vault(input.receiver, state.config.asset_a),
            definition: state.config.asset_a,
            expected_value: None,
        },
        Read {
            slot: Slot::ReserveB,
            account: vault(input.receiver, state.config.asset_b),
            definition: state.config.asset_b,
            expected_value: None,
        },
        Read {
            slot: Slot::RecipientA,
            account: op.recipient_a,
            definition: state.config.asset_a,
            expected_value: None,
        },
        Read {
            slot: Slot::RecipientB,
            account: op.recipient_b,
            definition: state.config.asset_b,
            expected_value: None,
        },
    ]
}
fn next_read(mut state: State) -> Response {
    let next = state.remaining.pop().expect("a read remains");
    let target = token(&state, next.account);
    state.expected = Some(next);
    write(&state).send_sync(target, &TokenMessage::ReadState)
}
fn value(state: &State, slot: Slot) -> u128 {
    *state
        .observed
        .get(&slot)
        .expect("all required reads completed")
}

fn settle(input: &ReceiveInput, mut state: State) -> Response {
    let op = state.operation.as_ref().expect("operation in progress");
    let supply = value(&state, Slot::Supply);
    assert_eq!(
        (
            value(&state, Slot::ReserveA),
            value(&state, Slot::ReserveB),
            supply
        ),
        (
            op.candidate.reserve_a,
            op.candidate.reserve_b,
            op.candidate.supply
        ),
        "stale redemption candidate"
    );
    assert!(
        op.shares <= value(&state, Slot::Source),
        "insufficient shares"
    );
    assert!(supply != 0 && op.shares <= supply, "invalid share supply");
    let a = op
        .shares
        .checked_mul(value(&state, Slot::ReserveA))
        .expect("redemption multiplication overflow")
        .checked_div(supply)
        .expect("nonzero share supply");
    let b = op
        .shares
        .checked_mul(value(&state, Slot::ReserveB))
        .expect("redemption multiplication overflow")
        .checked_div(supply)
        .expect("nonzero share supply");
    assert!(a != 0 && b != 0, "redemption rounds to zero");
    assert_eq!(
        (a, b),
        (op.candidate.output_a, op.candidate.output_b),
        "incorrect candidate output"
    );
    assert!(
        a >= op.min_a && b >= op.min_b,
        "redemption below requested minimum"
    );
    // Build checks from authenticated pre-operation values; caller-supplied balances
    // never participate. Asset definitions are distinct, so holdings cannot alias.
    let mut checks = reads(input, &state, op);
    for check in &mut checks {
        let before = value(&state, check.slot);
        check.expected_value = Some(match check.slot {
            Slot::Source | Slot::Supply => {
                before.checked_sub(op.shares).expect("share debit checked")
            }
            Slot::ReserveA => before.checked_sub(a).expect("reserve A debit checked"),
            Slot::ReserveB => before.checked_sub(b).expect("reserve B debit checked"),
            Slot::RecipientA => before.checked_add(a).expect("recipient A balance overflow"),
            Slot::RecipientB => before.checked_add(b).expect("recipient B balance overflow"),
        });
    }
    checks.reverse();
    let source = op.source;
    let amount = op.shares;
    let recipient_a = op.recipient_a;
    let recipient_b = op.recipient_b;
    state.phase = Phase::Settling;
    state.remaining = checks;
    write(&state)
        .send_sync(token(&state, source), &TokenMessage::Burn { amount })
        .send(vault_call(
            input,
            &state,
            state.config.asset_a,
            &TokenMessage::Transfer {
                to: recipient_a,
                amount: a,
            },
        ))
        .send(vault_call(
            input,
            &state,
            state.config.asset_b,
            &TokenMessage::Transfer {
                to: recipient_b,
                amount: b,
            },
        ))
        .send_sync(input.receiver, &Internal::BeginChecks)
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
        let state = State {
            config,
            phase: Phase::Ready,
            completed: 0,
            operation: None,
            expected: None,
            remaining: Vec::new(),
            observed: BTreeMap::new(),
        };
        return write(&state)
            .send(vault_call(
                input,
                &state,
                state.config.asset_a,
                &TokenMessage::InitializeHolding {
                    definition: state.config.asset_a,
                },
            ))
            .send(vault_call(
                input,
                &state,
                state.config.asset_b,
                &TokenMessage::InitializeHolding {
                    definition: state.config.asset_b,
                },
            ));
    }
    let mut state: State = borsh::from_slice(&input.pre_state).expect("redemption state decodes");
    if input.from == Some(input.receiver) {
        return match borsh::from_slice(&input.message).expect("internal command decodes") {
            Internal::BeginChecks => {
                assert_eq!(state.phase, Phase::Settling, "not settling");
                state.phase = Phase::Checking;
                next_read(state)
            }
            Internal::Complete => {
                assert_eq!(state.phase, Phase::Checking, "not checking");
                assert!(
                    state.expected.is_none() && state.remaining.is_empty(),
                    "checks incomplete"
                );
                assert!(
                    !state
                        .operation
                        .as_ref()
                        .expect("operation")
                        .reject_at_completion,
                    "example rejects after redemption"
                );
                state.phase = Phase::Ready;
                state.operation = None;
                state.observed.clear();
                state.completed = state.completed.checked_add(1).expect("completion overflow");
                write(&state)
            }
            Internal::AssertReady => {
                assert_eq!(state.phase, Phase::Ready, "redemption did not complete");
                Response::keep_state()
            }
        };
    }
    if state.phase != Phase::Ready {
        let expected = state
            .expected
            .take()
            .expect("no reply expected in this phase");
        assert_eq!(
            input.from,
            Some(token(&state, expected.account)),
            "unexpected read sender"
        );
        let reply: StateReply = borsh::from_slice(&input.message).expect("read reply decodes");
        let observed = match (expected.slot, TokenState::decode(&reply.state)) {
            (Slot::Supply, Some(TokenState::Definition(def))) => def.total_supply,
            (
                Slot::Source
                | Slot::ReserveA
                | Slot::ReserveB
                | Slot::RecipientA
                | Slot::RecipientB,
                Some(TokenState::Holding(holding)),
            ) => {
                assert_eq!(
                    holding.definition_id, expected.definition,
                    "unexpected token definition"
                );
                holding.balance
            }
            _ => panic!("unexpected token state type"),
        };
        if let Some(after) = expected.expected_value {
            assert_eq!(observed, after, "settlement state mismatch");
        } else {
            state.observed.insert(expected.slot, observed);
        }
        if !state.remaining.is_empty() {
            return next_read(state);
        }
        return if state.phase == Phase::Reading {
            settle(input, state)
        } else {
            write(&state).send_sync(input.receiver, &Internal::Complete)
        };
    }
    let Command::Redeem(op) = borsh::from_slice(&input.message).expect("command decodes") else {
        panic!("already initialized")
    };
    assert_ne!(op.shares, 0, "zero redemption");
    assert!(
        op.recipient_a != vault(input.receiver, state.config.asset_a)
            && op.recipient_b != vault(input.receiver, state.config.asset_b),
        "recipient cannot be its reserve"
    );
    state.remaining = reads(input, &state, &op);
    state.remaining.reverse();
    state.operation = Some(op);
    state.phase = Phase::Reading;
    next_read(state).send_sync(input.receiver, &Internal::AssertReady)
}
