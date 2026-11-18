//! Fixed-size arithmetic for the opt-in liquidity withdrawal.
//!
//! The program supplies all inputs and verifies the two outputs against its
//! proportional withdrawal rule before it emits any Token or oracle message.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::mul_div_floor;

const STACK_LIMIT: usize = 4;

/// Instructions for a withdrawal program. Existing tags must keep their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum WithdrawalOp {
    /// Padding. It does not modify the stack or the outputs.
    Nop,
    ReserveA,
    ReserveB,
    Liquidity,
    Supply,
    /// Consume x, y, denominator in stack order and push floor(x * y / denominator).
    MulDivFloor,
    /// Consume one stack value. The first output is A, and the second is B.
    Emit,
}

/// A client-supplied calculation with a fixed instruction budget.
/// It has no jumps, access to other actors, or authority to emit messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct WithdrawalPolicy {
    pub ops: [WithdrawalOp; 16],
}

#[derive(Debug, Clone, Copy)]
pub struct WithdrawalInputs {
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub liquidity: u128,
    pub supply: u128,
}

impl WithdrawalPolicy {
    /// Evaluate the instruction array on the supplied inputs.
    ///
    /// # Panics
    /// Panics on an invalid stack operation, invalid arithmetic, or incomplete outputs.
    #[must_use]
    pub fn evaluate(&self, inputs: WithdrawalInputs) -> (u128, u128) {
        let mut stack = Vec::with_capacity(STACK_LIMIT);
        let mut outputs = Vec::with_capacity(2);
        for op in self.ops {
            let value = match op {
                WithdrawalOp::Nop => continue,
                WithdrawalOp::ReserveA => inputs.reserve_a,
                WithdrawalOp::ReserveB => inputs.reserve_b,
                WithdrawalOp::Liquidity => inputs.liquidity,
                WithdrawalOp::Supply => inputs.supply,
                WithdrawalOp::MulDivFloor => {
                    let denominator = stack.pop().expect("policy stack underflow");
                    let y = stack.pop().expect("policy stack underflow");
                    let x = stack.pop().expect("policy stack underflow");
                    mul_div_floor(x, y, denominator)
                }
                WithdrawalOp::Emit => {
                    assert!(outputs.len() < 2, "policy emitted more than two outputs");
                    outputs.push(stack.pop().expect("policy stack underflow"));
                    continue;
                }
            };
            assert!(stack.len() < STACK_LIMIT, "policy stack limit exceeded");
            stack.push(value);
        }
        assert!(stack.is_empty(), "policy left unused stack values");
        let [a, b] = outputs.as_slice() else {
            panic!("policy must emit exactly two outputs");
        };
        (*a, *b)
    }
}
