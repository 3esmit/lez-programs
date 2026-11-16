//! A bounded arithmetic module. The interpreter has no actor, state or message API.
use borsh::{BorshDeserialize, BorshSerialize};

pub const MAX_INSTRUCTIONS: usize = 64;
const MAX_STACK: usize = 16;
const MAX_OUTPUTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Op {
    Input(u8),
    Constant(u128),
    Add,
    Subtract,
    MulDivFloor,
    Minimum,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Policy {
    pub code: Vec<Op>,
}

fn push(stack: &mut Vec<u128>, value: u128) {
    assert!(stack.len() < MAX_STACK, "policy stack limit");
    stack.push(value);
}

impl Policy {
    pub fn validate(&self) {
        assert!(
            !self.code.is_empty() && self.code.len() <= MAX_INSTRUCTIONS,
            "policy instruction limit"
        );
    }

    pub fn evaluate(&self, inputs: &[u128]) -> Vec<u128> {
        self.validate();
        let mut stack = Vec::new();
        let mut output = Vec::new();
        for op in &self.code {
            match *op {
                Op::Input(index) => push(
                    &mut stack,
                    *inputs.get(usize::from(index)).expect("policy input index"),
                ),
                Op::Constant(value) => push(&mut stack, value),
                Op::Add | Op::Subtract | Op::Minimum => {
                    let b = stack.pop().expect("policy stack underflow");
                    let a = stack.pop().expect("policy stack underflow");
                    let value = match op {
                        Op::Add => a.checked_add(b).expect("policy addition overflow"),
                        Op::Subtract => a
                            .checked_sub(b)
                            .expect("distribution exceeds received amount"),
                        Op::Minimum => a.min(b),
                        Op::Input(_) | Op::Constant(_) | Op::MulDivFloor | Op::Output => {
                            unreachable!("binary operation")
                        }
                    };
                    push(&mut stack, value);
                }
                Op::MulDivFloor => {
                    let divisor = stack.pop().expect("policy stack underflow");
                    let b = stack.pop().expect("policy stack underflow");
                    let a = stack.pop().expect("policy stack underflow");
                    let value = a
                        .checked_mul(b)
                        .expect("policy multiplication overflow")
                        .checked_div(divisor)
                        .expect("policy division by zero");
                    push(&mut stack, value);
                }
                Op::Output => {
                    assert!(output.len() < MAX_OUTPUTS, "policy output limit");
                    output.push(stack.pop().expect("policy stack underflow"));
                }
            }
        }
        assert!(stack.is_empty(), "policy left unused stack values");
        output
    }
}

pub fn payment() -> Policy {
    Policy {
        code: vec![
            Op::Input(1),
            Op::Output,
            Op::Input(2),
            Op::Output,
            Op::Input(0),
            Op::Input(1),
            Op::Subtract,
            Op::Input(2),
            Op::Subtract,
            Op::Output,
        ],
    }
}

pub fn redemption() -> Policy {
    // Inputs: shares, reserve A, reserve B, total supply.
    Policy {
        code: vec![
            Op::Input(0),
            Op::Input(1),
            Op::Input(3),
            Op::MulDivFloor,
            Op::Output,
            Op::Input(0),
            Op::Input(2),
            Op::Input(3),
            Op::MulDivFloor,
            Op::Output,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proportional_module_uses_all_current_inputs() {
        assert_eq!(
            redemption().evaluate(&[100, 1_500, 2_000, 900]),
            vec![166, 222]
        );
    }

    #[test]
    #[should_panic(expected = "policy instruction limit")]
    fn instruction_count_is_bounded() {
        Policy {
            code: vec![Op::Constant(0); MAX_INSTRUCTIONS + 1],
        }
        .evaluate(&[]);
    }

    #[test]
    #[should_panic(expected = "policy stack limit")]
    fn stack_is_bounded() {
        Policy {
            code: vec![Op::Constant(0); MAX_STACK + 1],
        }
        .evaluate(&[]);
    }

    #[test]
    #[should_panic(expected = "policy input index")]
    fn foreign_input_index_is_rejected() {
        Policy {
            code: vec![Op::Input(1), Op::Output],
        }
        .evaluate(&[5]);
    }

    #[test]
    #[should_panic(expected = "policy multiplication overflow")]
    fn arithmetic_cannot_wrap() {
        redemption().evaluate(&[2, u128::MAX, 10, 1]);
    }

    #[test]
    #[should_panic(expected = "policy division by zero")]
    fn invalid_divisor_is_rejected() {
        redemption().evaluate(&[1, 10, 20, 0]);
    }
}
