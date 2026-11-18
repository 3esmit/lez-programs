//! Schedule one authenticated read at a time and guard the final settlement step.
//!
//! The coordinator uses the existing AMM state variants as its continuation record.
//! The read step selects the message and reply type without adding a serialized field.

use amm_core::{AmmConfig, AmmState, Internal, Pending};
use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response, StateReply},
};
use token_core::{TokenHolding, TokenState};

pub(crate) enum ReadStep {
    Config,
    Holding,
    LpSupply,
}

pub(crate) enum ReadValue {
    Config(AmmConfig),
    Holding(Option<TokenHolding>),
    LpSupply(u128),
}

impl ReadStep {
    pub(crate) fn pending(pending: &Pending) -> Self {
        if pending.config.is_some() {
            Self::Holding
        } else {
            Self::Config
        }
    }

    pub(crate) fn ask(self, to: Actor, mut pending: Pending) -> Response {
        pending.expected_account = to.account_id;
        pending.expected_program = to.program_account_id;
        let pending = Box::new(pending);
        match self {
            Self::Config => Response::set_state(AmmState::Pending(pending).encode())
                .send_sync(to, &Internal::ReadState),
            Self::Holding => Response::set_state(AmmState::Pending(pending).encode())
                .send_sync(to, &token_core::Message::ReadState),
            Self::LpSupply => Response::set_state(AmmState::ReadingLpSupply(pending).encode())
                .send_sync(to, &token_core::Message::ReadState),
        }
    }

    pub(crate) fn receive(self, input: &ReceiveInput, pending: &Pending) -> ReadValue {
        let expected = Actor::new(pending.expected_account, pending.expected_program);
        assert_eq!(
            input.from,
            Some(expected),
            "unexpected AMM state reply sender"
        );
        let reply: StateReply = borsh::from_slice(&input.message).expect("state reply must decode");
        match self {
            Self::Config => {
                let Some(AmmState::Config(config)) = AmmState::decode(&reply.state) else {
                    panic!("AMM config must be initialized");
                };
                ReadValue::Config(config)
            }
            Self::Holding => ReadValue::Holding(match TokenState::decode(&reply.state) {
                Some(TokenState::Holding(holding)) => Some(holding),
                None => None,
                Some(TokenState::Definition(_)) => panic!("expected a token holding"),
            }),
            Self::LpSupply => {
                let Some(TokenState::Definition(definition)) = TokenState::decode(&reply.state)
                else {
                    panic!("LP token definition must be initialized");
                };
                ReadValue::LpSupply(definition.total_supply)
            }
        }
    }
}

pub(crate) fn settle(input: &ReceiveInput, next: AmmState, mut response: Response) -> Response {
    response.post_state = Some(AmmState::Settling(Box::new(next)).encode().into());
    response.send_sync(input.receiver, &Internal::Complete)
}

pub(crate) fn complete(input: &ReceiveInput, next: &AmmState) -> Response {
    assert_eq!(
        input.from,
        Some(input.receiver),
        "AMM operation is settling"
    );
    let message: Internal =
        borsh::from_slice(&input.message).expect("AMM internal message must decode");
    assert_eq!(
        message,
        Internal::Complete,
        "expected settlement completion"
    );
    Response::set_state(next.encode())
}
