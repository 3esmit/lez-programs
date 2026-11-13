//! One-use payment processor. Fixed destinations and limits are approved at initialization.
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use token_core::Message as TokenMessage;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub token_program: AccountId,
    pub definition: AccountId,
    pub payer: AccountId,
    pub beneficiary: AccountId,
    pub policy: Actor,
    pub budget: u128,
    pub max_pay: u128,
    pub max_burn: u128,
    pub min_refund: u128,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Initialize(Box<Config>),
    /// Debit only pay + burn. The unused budget never leaves the payer.
    Direct {
        reject: bool,
    },
    /// Spend a vault funded in a previous transaction, without the payer's credential.
    Prefunded {
        reject: bool,
    },
    /// Ask an ordinary actor for a quote, validate it, then spend the prefunded vault.
    WithPolicy {
        behavior: PolicyBehavior,
        reject: bool,
    },
    /// The processor owner may return funds only to the configured payer.
    Recover {
        amount: u128,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Phase {
    Ready,
    AwaitingPolicy,
    Settling,
    Spent,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct State {
    pub config: Config,
    pub phase: Phase,
    pub reject: bool,
}

/// Test modes show that a cooperative policy is not a runtime-enforced pure function.
#[derive(Debug, Clone, Copy, BorshSerialize, BorshDeserialize)]
pub enum PolicyBehavior {
    Quote,
    ExcessiveQuote,
    SpendPayer,
    NoReply,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct PolicyRequest {
    pub config: Config,
    pub behavior: PolicyBehavior,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Quote {
    pub pay: u128,
    pub burn: u128,
}

#[derive(BorshSerialize, BorshDeserialize)]
enum Internal {
    Complete,
    AssertFinished,
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
    Response::set_state(borsh::to_vec(state).expect("processor state encodes"))
}

fn token_call(
    input: &ReceiveInput,
    config: &Config,
    funded: bool,
    message: &TokenMessage,
) -> SyncMessage {
    let source = if funded {
        vault(input.receiver)
    } else {
        config.payer
    };
    let call = SyncMessage::new(Actor::new(source, config.token_program), message);
    if funded {
        call.with_pda_seeds([vault_seed(input.receiver.account_id)])
    } else {
        call
    }
}

fn settle(input: &ReceiveInput, mut state: State, funded: bool, quote: Quote) -> Response {
    assert!(
        quote.pay <= state.config.max_pay,
        "payment exceeds approved limit"
    );
    assert!(
        quote.burn <= state.config.max_burn,
        "burn exceeds approved limit"
    );
    let refund = state
        .config
        .budget
        .checked_sub(quote.pay)
        .and_then(|left| left.checked_sub(quote.burn))
        .expect("quote exceeds budget");
    assert!(
        refund >= state.config.min_refund,
        "refund below approved minimum"
    );
    state.phase = Phase::Settling;
    let mut response = write(&state)
        .send(token_call(
            input,
            &state.config,
            funded,
            &TokenMessage::Transfer {
                to: state.config.beneficiary,
                amount: quote.pay,
            },
        ))
        .send(token_call(
            input,
            &state.config,
            funded,
            &TokenMessage::Burn { amount: quote.burn },
        ));
    if funded {
        response = response.send(token_call(
            input,
            &state.config,
            true,
            &TokenMessage::Transfer {
                to: state.config.payer,
                amount: refund,
            },
        ));
    }
    response.send_sync(input.receiver, &Internal::Complete)
}

pub fn handle_message(input: &ReceiveInput) -> Response {
    if input.pre_state.is_empty() {
        assert!(
            input.is_authorized,
            "processor owner must authorize initialization"
        );
        let Command::Initialize(config) =
            borsh::from_slice(&input.message).expect("command decodes")
        else {
            panic!("processor must be initialized")
        };
        let config = *config;
        assert_ne!(
            config.policy.program_account_id, input.receiver.program_account_id,
            "policy must use another program"
        );
        assert!(
            config
                .budget
                .checked_sub(config.max_pay)
                .and_then(|left| left.checked_sub(config.max_burn))
                .is_some_and(|left| left >= config.min_refund),
            "invalid limits"
        );
        let state = State {
            config,
            phase: Phase::Ready,
            reject: false,
        };
        return write(&state).send(token_call(
            input,
            &state.config,
            true,
            &TokenMessage::InitializeHolding {
                definition: state.config.definition,
            },
        ));
    }
    let mut state: State = borsh::from_slice(&input.pre_state).expect("processor state decodes");
    if input.from == Some(input.receiver) {
        return match borsh::from_slice(&input.message).expect("internal command decodes") {
            Internal::Complete => {
                assert_eq!(state.phase, Phase::Settling, "no settlement in progress");
                assert!(!state.reject, "example rejects after token effects");
                state.phase = Phase::Spent;
                write(&state)
            }
            Internal::AssertFinished => {
                assert_eq!(
                    state.phase,
                    Phase::Spent,
                    "policy did not complete settlement"
                );
                Response::keep_state()
            }
        };
    }
    if input.from == Some(state.config.policy) {
        assert_eq!(
            state.phase,
            Phase::AwaitingPolicy,
            "unexpected policy reply"
        );
        let quote = borsh::from_slice(&input.message).expect("quote decodes");
        return settle(input, state, true, quote);
    }
    let command: Command = borsh::from_slice(&input.message).expect("command decodes");
    if let Command::Recover { amount } = command {
        assert!(
            input.is_authorized,
            "processor owner must authorize recovery"
        );
        assert!(
            matches!(state.phase, Phase::Ready | Phase::Spent),
            "operation in progress"
        );
        return Response::keep_state().send(token_call(
            input,
            &state.config,
            true,
            &TokenMessage::Transfer {
                to: state.config.payer,
                amount,
            },
        ));
    }
    assert_eq!(
        state.phase,
        Phase::Ready,
        "operation already started or spent"
    );
    match command {
        Command::Initialize(_) => panic!("processor already initialized"),
        Command::Recover { .. } => unreachable!("recovery handled above"),
        Command::Direct { reject } | Command::Prefunded { reject } => {
            let funded = matches!(command, Command::Prefunded { .. });
            state.reject = reject;
            let quote = Quote {
                pay: state.config.max_pay,
                burn: state.config.max_burn,
            };
            settle(input, state, funded, quote)
        }
        Command::WithPolicy { behavior, reject } => {
            state.phase = Phase::AwaitingPolicy;
            state.reject = reject;
            // No vault grant is issued to this external call. The execution transaction
            // must omit the payer's credential; the LEZ runtime does not attenuate it.
            write(&state)
                .send_sync(
                    state.config.policy,
                    &PolicyRequest {
                        config: state.config.clone(),
                        behavior,
                    },
                )
                .send_sync(input.receiver, &Internal::AssertFinished)
        }
    }
}

/// Ordinary policy actor: its state write demonstrates the absence of enforced purity.
pub fn policy(input: &ReceiveInput) -> Response {
    if input.from.is_none() {
        assert!(
            input.is_authorized && input.pre_state.is_empty(),
            "policy initialization requires its owner"
        );
        let (): () = borsh::from_slice(&input.message).expect("empty initialization command");
        return Response::set_state(vec![0_u8]);
    }
    let request: PolicyRequest = borsh::from_slice(&input.message).expect("policy request decodes");
    let from = input.from.expect("policy needs a reply actor");
    let mut response = Response::set_state(vec![1_u8]);
    let mut pay = request.config.max_pay;
    match request.behavior {
        PolicyBehavior::Quote => {}
        PolicyBehavior::ExcessiveQuote => pay = pay.checked_add(1).expect("quote overflow"),
        PolicyBehavior::SpendPayer => {
            response = response.send_sync(
                Actor::new(request.config.payer, request.config.token_program),
                &TokenMessage::Transfer {
                    to: request.config.beneficiary,
                    amount: 1,
                },
            );
        }
        PolicyBehavior::NoReply => return response,
    }
    response.send_sync(
        from,
        &Quote {
            pay,
            burn: request.config.max_burn,
        },
    )
}
