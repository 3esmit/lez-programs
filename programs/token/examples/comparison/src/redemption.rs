//! Validate a client redemption plan with conditional Token effects and final assertions.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use sha2::{Digest as _, Sha256};
use token_core::Message as TokenMessage;

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
    pub source_balance: u128,
    pub recipient_a_balance: u128,
    pub recipient_b_balance: u128,
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
    Settling,
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct State {
    pub config: Config,
    pub phase: Phase,
    pub completed: u64,
    pub reject: bool,
}
#[derive(BorshSerialize, BorshDeserialize)]
enum Internal {
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
    Response::set_state(borsh::to_vec(state).expect("state encodes"))
}
fn vault_call(
    input: &ReceiveInput,
    config: &Config,
    asset: AccountId,
    message: &TokenMessage,
) -> SyncMessage {
    SyncMessage::new(
        Actor::new(vault(input.receiver, asset), config.token_program),
        message,
    )
    .with_pda_seeds([vault_seed(input.receiver.account_id, asset)])
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
            reject: false,
        };
        return write(&state)
            .send(vault_call(
                input,
                &state.config,
                state.config.asset_a,
                &TokenMessage::InitializeHolding {
                    definition: state.config.asset_a,
                },
            ))
            .send(vault_call(
                input,
                &state.config,
                state.config.asset_b,
                &TokenMessage::InitializeHolding {
                    definition: state.config.asset_b,
                },
            ));
    }
    let mut state: State = borsh::from_slice(&input.pre_state).expect("state decodes");
    if input.from == Some(input.receiver) {
        return match borsh::from_slice(&input.message).expect("internal decodes") {
            Internal::Complete => {
                assert_eq!(state.phase, Phase::Settling, "not settling");
                assert!(!state.reject, "example rejects after redemption");
                state.phase = Phase::Ready;
                state.completed = state.completed.checked_add(1).expect("completion overflow");
                write(&state)
            }
            Internal::AssertReady => {
                assert_eq!(state.phase, Phase::Ready, "redemption did not complete");
                Response::keep_state()
            }
        };
    }
    assert_eq!(state.phase, Phase::Ready, "redemption is settling");
    let Command::Redeem(op) = borsh::from_slice(&input.message).expect("command decodes") else {
        panic!("already initialized")
    };
    assert_ne!(op.shares, 0, "zero redemption");
    assert!(
        op.recipient_a != vault(input.receiver, state.config.asset_a)
            && op.recipient_b != vault(input.receiver, state.config.asset_b),
        "recipient cannot be its reserve"
    );
    let quote = &op.candidate;
    assert!(op.shares <= quote.source_balance, "insufficient shares");
    assert!(
        quote.supply != 0 && op.shares <= quote.supply,
        "invalid share supply"
    );
    let a = op
        .shares
        .checked_mul(quote.reserve_a)
        .expect("redemption multiplication overflow")
        .checked_div(quote.supply)
        .expect("nonzero supply");
    let b = op
        .shares
        .checked_mul(quote.reserve_b)
        .expect("redemption multiplication overflow")
        .checked_div(quote.supply)
        .expect("nonzero supply");
    assert_eq!(
        (a, b),
        (quote.output_a, quote.output_b),
        "incorrect candidate output"
    );
    assert!(a != 0 && b != 0, "redemption rounds to zero");
    assert!(
        a >= op.min_a && b >= op.min_b,
        "redemption below requested minimum"
    );
    state.phase = Phase::Settling;
    state.reject = op.reject_at_completion;
    let config = &state.config;
    let token = |account| Actor::new(account, config.token_program);
    let mut response = write(&state).send_sync(
        token(op.source),
        &TokenMessage::BurnChecked {
            definition: config.shares,
            expected_balance: quote.source_balance,
            expected_supply: quote.supply,
            amount: op.shares,
        },
    );
    for (asset, reserve, recipient, amount, recipient_before) in [
        (
            config.asset_a,
            quote.reserve_a,
            op.recipient_a,
            a,
            quote.recipient_a_balance,
        ),
        (
            config.asset_b,
            quote.reserve_b,
            op.recipient_b,
            b,
            quote.recipient_b_balance,
        ),
    ] {
        response = response
            .send(vault_call(
                input,
                config,
                asset,
                &TokenMessage::TransferChecked {
                    definition: asset,
                    expected_balance: reserve,
                    to: recipient,
                    amount,
                },
            ))
            .send_sync(
                token(vault(input.receiver, asset)),
                &TokenMessage::AssertHolding {
                    definition: asset,
                    balance: reserve.checked_sub(amount).expect("reserve debit"),
                },
            )
            .send_sync(
                token(recipient),
                &TokenMessage::AssertHolding {
                    definition: asset,
                    balance: recipient_before
                        .checked_add(amount)
                        .expect("recipient overflow"),
                },
            );
    }
    response
        .send_sync(
            token(op.source),
            &TokenMessage::AssertHolding {
                definition: config.shares,
                balance: quote
                    .source_balance
                    .checked_sub(op.shares)
                    .expect("share debit"),
            },
        )
        .send_sync(
            token(config.shares),
            &TokenMessage::AssertSupply {
                supply: quote.supply.checked_sub(op.shares).expect("supply debit"),
            },
        )
        .send_sync(input.receiver, &Internal::Complete)
        .send_sync(input.receiver, &Internal::AssertReady)
}
