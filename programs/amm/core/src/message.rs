use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::AccountId;

use crate::AmmConfig;

/// Public actor messages. NewDefinition goes to the config actor; other pool operations
/// go to the pool actor. Initialization and admin
/// operations go to the authorized owner's/authority's AMM actor, which forwards them.
/// Token destinations must be declared so credits settle in this transaction. All holdings
/// must exist, except the initial LP holding (created with the pool).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// Sent to the authorized namespace owner's actor. Creates the config PDA derived
    /// from that owner and `nonce`; deployment ids and fees are immutable afterward.
    Initialize {
        nonce: [u8; 32],
        config: AmmConfig,
    },
    /// Sent to the authorized current admin's actor, which forwards to `config`.
    UpdateConfig {
        config: AccountId,
        new_authority: AccountId,
    },
    /// Sent to `config`, which admits the derived pool with its PDA seed. Both deposit
    /// holdings and the fresh LP holding authorize; the permanent minimum LP lock is
    /// deducted from the initial supply before the transaction finishes.
    NewDefinition {
        config: AccountId,
        definition_token_a_id: AccountId,
        definition_token_b_id: AccountId,
        user_holding_a: AccountId,
        user_holding_b: AccountId,
        user_holding_lp: AccountId,
        token_a_amount: u128,
        token_b_amount: u128,
        /// Last valid block timestamp in milliseconds (inclusive).
        deadline: u64,
    },
    /// Both deposit holdings authorize. The existing LP holding receives minted LPs.
    AddLiquidity {
        user_holding_a: AccountId,
        user_holding_b: AccountId,
        user_holding_lp: AccountId,
        min_amount_liquidity: u128,
        max_amount_to_add_token_a: u128,
        max_amount_to_add_token_b: u128,
        deadline: u64,
    },
    /// The LP holding authorizes its burn; the two output holdings only receive.
    RemoveLiquidity {
        user_holding_a: AccountId,
        user_holding_b: AccountId,
        user_holding_lp: AccountId,
        remove_liquidity_amount: u128,
        min_amount_to_remove_token_a: u128,
        min_amount_to_remove_token_b: u128,
        deadline: u64,
    },
    /// The input holding authorizes. Its token definition selects the direction.
    SwapExactInput {
        user_input_holding: AccountId,
        user_output_holding: AccountId,
        swap_amount_in: u128,
        min_amount_out: u128,
        deadline: u64,
    },
    /// Like exact input, with a fixed output and a maximum debit instead.
    SwapExactOutput {
        user_input_holding: AccountId,
        user_output_holding: AccountId,
        exact_amount_out: u128,
        max_amount_in: u128,
        deadline: u64,
    },
    /// Permissionless: incorporate direct vault donations into reserves and notify the oracle.
    SyncReserves,
    CreatePriceObservations {
        window_duration: u64,
    },
    CreateOraclePriceAccount {
        window_duration: u64,
    },
    /// Sent to the authorized current admin's actor. The source is derived from
    /// `(config, token)`; `destination` must already hold that token.
    WithdrawProtocolFees {
        config: AccountId,
        token: AccountId,
        destination: AccountId,
        amount: u128,
    },
    /// Reply to the sender with a `lee_core::program::StateReply`. Busy actors reject reads.
    ReadState,
    /// Opt in to a strict removal quote. Token checks the live holdings and share supply
    /// when it settles the operation. The LP holding authorizes the burn.
    RemoveLiquidityChecked {
        request: Box<CheckedRemoval>,
    },
    /// Check initial source balances, fund both new vaults, and lock the minimum LP.
    NewDefinitionChecked {
        request: Box<CheckedPoolCreation>,
    },
    /// Check the quote, deposits, LP supply and mint receipt.
    AddLiquidityChecked {
        request: Box<CheckedAdd>,
    },
    /// Check an exact input swap quote and acknowledge each transfer credit.
    SwapExactInputChecked {
        user_input_holding: AccountId,
        user_output_holding: AccountId,
        swap_amount_in: u128,
        min_amount_out: u128,
        deadline: u64,
        quote: Box<SwapQuote>,
    },
    /// Check an exact output swap quote and acknowledge each transfer credit.
    SwapExactOutputChecked {
        user_input_holding: AccountId,
        user_output_holding: AccountId,
        exact_amount_out: u128,
        max_amount_in: u128,
        deadline: u64,
        quote: Box<SwapQuote>,
    },
    /// Adopt the supplied reserve balances only if the actual vaults match.
    SyncReservesChecked {
        quote: PoolQuote,
    },
    /// The admin authorizes the same withdrawal with exact source and destination balances.
    WithdrawProtocolFeesChecked {
        config: AccountId,
        token: AccountId,
        destination: AccountId,
        amount: u128,
        fee_balance: u128,
        destination_balance: u128,
    },
    /// Create the observations only when the vaults match the supplied backing.
    CreatePriceObservationsChecked {
        window_duration: u64,
        quote: PoolQuote,
    },
    /// Create the price account only when the vaults match the supplied backing.
    CreateOraclePriceAccountChecked {
        window_duration: u64,
        quote: PoolQuote,
    },
}

/// A proportional withdrawal request with exact expected state.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CheckedRemoval {
    pub user_holding_a: AccountId,
    pub user_holding_b: AccountId,
    pub user_holding_lp: AccountId,
    pub liquidity: u128,
    pub min_a: u128,
    pub min_b: u128,
    /// Last valid block timestamp in milliseconds (inclusive).
    pub deadline: u64,
    pub quote: RemovalQuote,
}

/// The recorded reserves determine the price. The vault balances are separate
/// conditions so an unsynced donation can remain in the vault.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct RemovalQuote {
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub lp_supply: u128,
    pub lp_balance: u128,
    pub vault_a_balance: u128,
    pub vault_b_balance: u128,
    pub recipient_a_balance: u128,
    pub recipient_b_balance: u128,
    pub amount_a: u128,
    pub amount_b: u128,
}

/// Pool accounting and Token custody are separate conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PoolQuote {
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub vault_a_balance: u128,
    pub vault_b_balance: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CheckedPoolCreation {
    pub config: AccountId,
    pub definition_token_a_id: AccountId,
    pub definition_token_b_id: AccountId,
    pub user_holding_a: AccountId,
    pub user_holding_b: AccountId,
    pub user_holding_lp: AccountId,
    pub token_a_amount: u128,
    pub token_b_amount: u128,
    pub source_a_balance: u128,
    pub source_b_balance: u128,
    /// Last valid block timestamp in milliseconds (inclusive).
    pub deadline: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CheckedAdd {
    pub user_holding_a: AccountId,
    pub user_holding_b: AccountId,
    pub user_holding_lp: AccountId,
    pub min_amount_liquidity: u128,
    pub max_amount_to_add_token_a: u128,
    pub max_amount_to_add_token_b: u128,
    /// Last valid block timestamp in milliseconds (inclusive).
    pub deadline: u64,
    pub quote: AddQuote,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AddQuote {
    pub pool: PoolQuote,
    pub lp_supply: u128,
    pub source_a_balance: u128,
    pub source_b_balance: u128,
    pub recipient_lp_balance: u128,
    pub amount_a: u128,
    pub amount_b: u128,
    pub liquidity: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SwapQuote {
    pub pool: PoolQuote,
    pub input_definition: AccountId,
    pub source_balance: u128,
    pub recipient_balance: u128,
    pub amount_in: u128,
    pub amount_out: u128,
}

/// Decoded only for messages from this AMM deployment. No generic forwarding or arbitrary
/// seed grants: each privileged message is constructed by an authorized public handler.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Internal {
    InitializeConfig {
        nonce: [u8; 32],
        config: AmmConfig,
    },
    UpdateConfig {
        new_authority: AccountId,
    },
    WithdrawProtocolFees {
        token: AccountId,
        destination: AccountId,
        amount: u128,
    },
    ReadState,
    Complete,
    NewPool {
        request: Box<Message>,
    },
    WithdrawProtocolFeesChecked {
        token: AccountId,
        destination: AccountId,
        amount: u128,
        fee_balance: u128,
        destination_balance: u128,
    },
}
