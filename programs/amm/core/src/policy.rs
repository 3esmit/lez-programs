//! Fixed-size arithmetic for opt-in pool operations.
//!
//! The AMM supplies all inputs and checks each output against its canonical
//! formula before it emits any Token or oracle message.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{isqrt_product, mul_div_ceil, mul_div_floor, FEE_BPS_DENOMINATOR};

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
        let ops = self.ops.map(|op| match op {
            WithdrawalOp::Nop => CalculationOp::Nop,
            WithdrawalOp::ReserveA => CalculationOp::Input { index: 0 },
            WithdrawalOp::ReserveB => CalculationOp::Input { index: 1 },
            WithdrawalOp::Liquidity => CalculationOp::Input { index: 2 },
            WithdrawalOp::Supply => CalculationOp::Input { index: 3 },
            WithdrawalOp::MulDivFloor => CalculationOp::MulDivFloor,
            WithdrawalOp::Emit => CalculationOp::Emit,
        });
        let [a, b] = evaluate(
            ops,
            &[
                inputs.reserve_a,
                inputs.reserve_b,
                inputs.liquidity,
                inputs.supply,
            ],
        );
        (a, b)
    }
}

/// Instructions for the new pool calculation paths. Input order is defined by
/// the AMM message that carries the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum CalculationOp {
    Nop,
    Input {
        index: u8,
    },
    FeeDenominator,
    Add,
    Subtract,
    Minimum,
    MulDivFloor,
    MulDivCeil,
    SqrtProduct,
    Emit,
    /// Read a value this program already emitted.
    Output {
        index: u8,
    },
}

/// A fixed-size arithmetic program with no jumps, state access, or message
/// effects. Unused instruction slots contain `CalculationOp::Nop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CalculationPolicy {
    pub ops: [CalculationOp; 32],
}

impl CalculationPolicy {
    /// Check the client calculation against the AMM's own results.
    ///
    /// # Panics
    /// Panics if the calculation is invalid or any result differs.
    pub fn assert_matches<const N: usize>(&self, inputs: &[u128], expected: [u128; N]) {
        assert_eq!(
            evaluate(self.ops, inputs),
            expected,
            "policy differs from canonical pool calculation"
        );
    }
}

fn pop(stack: &mut Vec<u128>) -> u128 {
    stack.pop().expect("policy stack underflow")
}

fn evaluate<const N: usize>(
    ops: impl IntoIterator<Item = CalculationOp>,
    inputs: &[u128],
) -> [u128; N] {
    let mut stack = Vec::with_capacity(STACK_LIMIT);
    let mut outputs = Vec::with_capacity(N.min(3));
    for op in ops {
        let value = match op {
            CalculationOp::Nop => continue,
            CalculationOp::Input { index } => *inputs
                .get(usize::from(index))
                .expect("policy input index out of bounds"),
            CalculationOp::Output { index } => *outputs
                .get(usize::from(index))
                .expect("policy output index out of bounds"),
            CalculationOp::FeeDenominator => FEE_BPS_DENOMINATOR,
            CalculationOp::Add => {
                let rhs = pop(&mut stack);
                pop(&mut stack)
                    .checked_add(rhs)
                    .expect("policy addition overflow")
            }
            CalculationOp::Subtract => {
                let rhs = pop(&mut stack);
                pop(&mut stack)
                    .checked_sub(rhs)
                    .expect("policy subtraction underflow")
            }
            CalculationOp::Minimum => {
                let rhs = pop(&mut stack);
                pop(&mut stack).min(rhs)
            }
            CalculationOp::MulDivFloor | CalculationOp::MulDivCeil => {
                let denominator = pop(&mut stack);
                let y = pop(&mut stack);
                let x = pop(&mut stack);
                if op == CalculationOp::MulDivFloor {
                    mul_div_floor(x, y, denominator)
                } else {
                    mul_div_ceil(x, y, denominator)
                }
            }
            CalculationOp::SqrtProduct => {
                let rhs = pop(&mut stack);
                isqrt_product(pop(&mut stack), rhs)
            }
            CalculationOp::Emit => {
                assert!(outputs.len() < N.min(3), "policy emitted too many outputs");
                outputs.push(pop(&mut stack));
                continue;
            }
        };
        assert!(stack.len() < STACK_LIMIT, "policy stack limit exceeded");
        stack.push(value);
    }
    assert!(stack.is_empty(), "policy left unused stack values");
    outputs
        .try_into()
        .expect("policy must emit every required output")
}
