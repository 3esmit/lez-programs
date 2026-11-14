//! Payment workflow with client proposals and conditional Token effects.
//! Each Token actor checks its expected state before applying its effect.
//! Credit acknowledgements and final assertions require immediate settlement.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use token_core::Message as TokenMessage;

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
    pub candidate: Candidate,
    /// Demonstration switch: fail after the transfers, burn and final balance checks.
    pub reject_at_completion: bool,
}

/// A client quote is untrusted until Token actors check the expected values.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Candidate {
    pub source_balance: u128,
    pub vault_balance: u128,
    pub beneficiary_balance: u128,
    pub refund: u128,
    pub supply: u128,
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
    Settling { next: Data, reject: bool },
}

#[derive(BorshSerialize, BorshDeserialize)]
enum Internal {
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
fn write(state: &State) -> Response {
    Response::set_state(borsh::to_vec(state).expect("payment state encodes"))
}
fn vault_call(input: &ReceiveInput, data: &Data, message: &TokenMessage) -> SyncMessage {
    SyncMessage::new(
        Actor::new(vault(input.receiver), data.token_program),
        message,
    )
    .with_pda_seeds([vault_seed(input.receiver.account_id)])
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
        return write(&State::Ready(data.clone())).send(vault_call(
            input,
            &data,
            &TokenMessage::InitializeHolding { definition },
        ));
    }
    let state: State = borsh::from_slice(&input.pre_state).expect("payment state decodes");
    if input.from == Some(input.receiver) {
        return match borsh::from_slice(&input.message).expect("internal decodes") {
            Internal::Complete => {
                let State::Settling { next, reject } = state else {
                    panic!("not settling")
                };
                assert!(!reject, "example rejects after token effects");
                write(&State::Ready(next))
            }
            Internal::AssertReady => {
                assert!(matches!(state, State::Ready(_)), "payment did not complete");
                Response::keep_state()
            }
        };
    }
    let State::Ready(mut data) = state else {
        panic!("payment is settling")
    };
    let Command::Process(op) = borsh::from_slice(&input.message).expect("command decodes") else {
        panic!("already initialized")
    };
    let custody = vault(input.receiver);
    assert_ne!(op.source, custody, "source must not be the protocol vault");
    let refund = op
        .amount
        .checked_sub(op.pay)
        .and_then(|left| left.checked_sub(op.burn))
        .expect("distribution exceeds received amount");
    assert_eq!(refund, op.candidate.refund, "incorrect candidate refund");
    let mut balances = BTreeMap::new();
    for (account, value) in [
        (op.source, op.candidate.source_balance),
        (custody, op.candidate.vault_balance),
        (data.beneficiary, op.candidate.beneficiary_balance),
    ] {
        if let Some(previous) = balances.insert(account, value) {
            assert_eq!(previous, value, "aliased candidate values disagree");
        }
    }
    let token_program = data.token_program;
    let definition = data.definition;
    data.completed = data.completed.checked_add(1).expect("completion overflow");
    data.last_received = op.amount;
    let mut response = write(&State::Settling {
        next: data.clone(),
        reject: op.reject_at_completion,
    });
    for (&account, &balance) in &balances {
        response = response.send_sync(
            Actor::new(account, token_program),
            &TokenMessage::AssertHolding {
                definition,
                balance,
            },
        );
    }
    for (from, to, amount) in [
        (op.source, custody, op.amount),
        (custody, data.beneficiary, op.pay),
    ] {
        let expected_balance = *balances.get(&from).expect("known source");
        *balances.get_mut(&from).expect("known source") = expected_balance
            .checked_sub(amount)
            .expect("insufficient balance");
        let target = balances.get_mut(&to).expect("known destination");
        *target = target.checked_add(amount).expect("balance overflow");
        let message = TokenMessage::TransferChecked {
            definition,
            expected_balance,
            to,
            amount,
        };
        response = if from == custody {
            response.send(vault_call(input, &data, &message))
        } else {
            response.send_sync(Actor::new(from, token_program), &message)
        };
    }
    let balance = balances.get_mut(&custody).expect("vault");
    let expected_balance = *balance;
    *balance = balance.checked_sub(op.burn).expect("insufficient balance");
    response = response.send(vault_call(
        input,
        &data,
        &TokenMessage::BurnChecked {
            definition,
            expected_balance,
            expected_supply: op.candidate.supply,
            amount: op.burn,
        },
    ));
    let expected_balance = *balance;
    *balance = balance.checked_sub(refund).expect("insufficient balance");
    let payer = balances.get_mut(&op.source).expect("payer");
    *payer = payer.checked_add(refund).expect("balance overflow");
    response = response.send(vault_call(
        input,
        &data,
        &TokenMessage::TransferChecked {
            definition,
            expected_balance,
            to: op.source,
            amount: refund,
        },
    ));
    for (account, balance) in balances {
        response = response.send_sync(
            Actor::new(account, token_program),
            &TokenMessage::AssertHolding {
                definition,
                balance,
            },
        );
    }
    response
        .send_sync(
            Actor::new(definition, token_program),
            &TokenMessage::AssertSupply {
                supply: op
                    .candidate
                    .supply
                    .checked_sub(op.burn)
                    .expect("burn exceeds supply"),
            },
        )
        .send_sync(input.receiver, &Internal::Complete)
        .send_sync(input.receiver, &Internal::AssertReady)
}
