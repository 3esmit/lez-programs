//! Reusable, bounded orchestration of authenticated Token reads and typed effects.
//! Recipes are constructed by the consumer, never accepted as arbitrary user bytecode.
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, StateReply, SyncMessage},
};
use token_core::{Message, TokenState};

const MAX_STEPS: usize = 96;
const MAX_REGISTERS: usize = 24;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Step {
    Initialize {
        actor: Actor,
        definition: AccountId,
        seed: PdaSeed,
    },
    Read {
        actor: Actor,
        definition: AccountId,
        supply: bool,
        register: u8,
    },
    Constant {
        register: u8,
        value: u128,
    },
    Add {
        left: u8,
        right: u8,
        out: u8,
    },
    Subtract {
        left: u8,
        right: u8,
        out: u8,
    },
    MulDiv {
        left: u8,
        right: u8,
        divisor: u8,
        out: u8,
    },
    AtLeast {
        left: u8,
        right: u8,
        failure: Failure,
    },
    Equal {
        left: u8,
        right: u8,
    },
    Positive {
        register: u8,
    },
    Transfer {
        from: Actor,
        to: AccountId,
        amount: u8,
        seed: Option<PdaSeed>,
    },
    Burn {
        from: Actor,
        amount: u8,
        seed: Option<PdaSeed>,
    },
}

#[derive(Debug, Clone, Copy, BorshSerialize, BorshDeserialize)]
pub enum Failure {
    Shares,
    Supply,
    Minimum,
}
impl Failure {
    fn message(self) -> &'static str {
        match self {
            Self::Shares => "insufficient shares",
            Self::Supply => "invalid share supply",
            Self::Minimum => "redemption below requested minimum",
        }
    }
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
enum Waiting {
    None,
    Read {
        actor: Actor,
        definition: AccountId,
        supply: bool,
        register: u8,
    },
    Effect,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Machine {
    steps: Vec<Step>,
    cursor: u16,
    registers: BTreeMap<u8, u128>,
    waiting: Waiting,
}

pub enum Action {
    Read(SyncMessage),
    Effect(SyncMessage),
    Finished,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub enum Control {
    Resume,
    AssertReady,
    AssertFunded,
    Cancel { id: u64 },
}

impl Machine {
    pub fn new(steps: Vec<Step>) -> Self {
        assert!(
            !steps.is_empty() && steps.len() <= MAX_STEPS,
            "recipe step limit"
        );
        Self {
            steps,
            cursor: 0,
            registers: BTreeMap::new(),
            waiting: Waiting::None,
        }
    }

    fn get(&self, register: u8) -> u128 {
        *self
            .registers
            .get(&register)
            .expect("uninitialized recipe register")
    }

    fn put(&mut self, register: u8, value: u128) {
        assert!(
            usize::from(register) < MAX_REGISTERS,
            "recipe register limit"
        );
        self.registers.insert(register, value);
    }

    pub fn accept_reply(&mut self, input: &ReceiveInput) {
        let Waiting::Read {
            actor,
            definition,
            supply,
            register,
        } = self.waiting
        else {
            panic!("no recipe read expected")
        };
        assert_eq!(input.from, Some(actor), "unexpected recipe read sender");
        let reply: StateReply = borsh::from_slice(&input.message).expect("state reply decodes");
        let value = match (supply, TokenState::decode(&reply.state)) {
            (false, Some(TokenState::Holding(holding))) => {
                assert_eq!(
                    holding.definition_id, definition,
                    "unexpected token definition"
                );
                holding.balance
            }
            (true, Some(TokenState::Definition(definition_state))) => {
                assert_eq!(actor.account_id, definition, "unexpected supply actor");
                definition_state.total_supply
            }
            _ => panic!("expected initialized token holding or definition"),
        };
        self.put(register, value);
        self.waiting = Waiting::None;
    }

    pub fn resume_effect(&mut self) {
        assert!(
            matches!(self.waiting, Waiting::Effect),
            "no recipe effect in progress"
        );
        self.waiting = Waiting::None;
    }

    pub fn advance(&mut self) -> Action {
        assert!(matches!(self.waiting, Waiting::None), "recipe is waiting");
        while let Some(step) = self.steps.get(usize::from(self.cursor)).cloned() {
            self.cursor = self.cursor.checked_add(1).expect("recipe cursor overflow");
            match step {
                Step::Initialize {
                    actor,
                    definition,
                    seed,
                } => {
                    self.waiting = Waiting::Effect;
                    return Action::Effect(
                        SyncMessage::new(actor, &Message::InitializeHolding { definition })
                            .with_pda_seeds([seed]),
                    );
                }
                Step::Read {
                    actor,
                    definition,
                    supply,
                    register,
                } => {
                    self.waiting = Waiting::Read {
                        actor,
                        definition,
                        supply,
                        register,
                    };
                    return Action::Read(SyncMessage::new(actor, &Message::ReadState));
                }
                Step::Constant { register, value } => self.put(register, value),
                Step::Add { left, right, out } => self.put(
                    out,
                    self.get(left)
                        .checked_add(self.get(right))
                        .expect("balance overflow"),
                ),
                Step::Subtract { left, right, out } => self.put(
                    out,
                    self.get(left)
                        .checked_sub(self.get(right))
                        .expect("insufficient balance"),
                ),
                Step::MulDiv {
                    left,
                    right,
                    divisor,
                    out,
                } => self.put(
                    out,
                    self.get(left)
                        .checked_mul(self.get(right))
                        .expect("redemption multiplication overflow")
                        .checked_div(self.get(divisor))
                        .expect("invalid share supply"),
                ),
                Step::AtLeast {
                    left,
                    right,
                    failure,
                } => assert!(self.get(left) >= self.get(right), "{}", failure.message()),
                Step::Equal { left, right } => assert_eq!(
                    self.get(left),
                    self.get(right),
                    "required token credit did not settle immediately"
                ),
                Step::Positive { register } => {
                    assert_ne!(self.get(register), 0, "redemption rounds to zero")
                }
                Step::Transfer {
                    from,
                    to,
                    amount,
                    seed,
                } => {
                    self.waiting = Waiting::Effect;
                    let call = SyncMessage::new(
                        from,
                        &Message::Transfer {
                            to,
                            amount: self.get(amount),
                        },
                    );
                    return Action::Effect(match seed {
                        Some(seed) => call.with_pda_seeds([seed]),
                        None => call,
                    });
                }
                Step::Burn { from, amount, seed } => {
                    self.waiting = Waiting::Effect;
                    let call = SyncMessage::new(
                        from,
                        &Message::Burn {
                            amount: self.get(amount),
                        },
                    );
                    return Action::Effect(match seed {
                        Some(seed) => call.with_pda_seeds([seed]),
                        None => call,
                    });
                }
            }
        }
        Action::Finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[should_panic(expected = "recipe step limit")]
    fn bounds_recipe_size() {
        Machine::new(vec![
            Step::Constant {
                register: 0,
                value: 0
            };
            97
        ]);
    }
    #[test]
    #[should_panic(expected = "recipe register limit")]
    fn bounds_register_addresses() {
        Machine::new(vec![Step::Constant {
            register: 24,
            value: 0,
        }])
        .advance();
    }
    #[test]
    #[should_panic(expected = "uninitialized recipe register")]
    fn missing_results_are_not_zero() {
        Machine::new(vec![Step::Positive { register: 0 }]).advance();
    }
    #[test]
    #[should_panic(expected = "no recipe effect in progress")]
    fn cannot_resume_an_effect_that_was_not_emitted() {
        Machine::new(vec![Step::Constant {
            register: 0,
            value: 0,
        }])
        .resume_effect();
    }
}
