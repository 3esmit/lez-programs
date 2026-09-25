//! Print stablecoin PDAs: the five program-wide singletons, or a position and its
//! collateral vault.
//!
//! Usage:
//!   cargo run -q -p stablecoin_program --example stablecoin_pdas -- <stablecoin_account_id>
//! globals
//!   cargo run -q -p stablecoin_program --example stablecoin_pdas --
//! <stablecoin_account_id> <owner> <position_nonce>
//!
//! Since LEZ v0.2.5 a program is addressed by the account id of its deployed `ProgramHeader`,
//! not by its ImageID, so `stablecoin_account_id` is a base58 account id, as is `owner`.
//! `position_nonce` is a u64.
//!
//! `globals` prints the accounts `initialize_program` creates. The stablecoin
//! definition is needed *before* initialization: the market price oracle must
//! already quote it as its base asset.

use std::str::FromStr;

use lee_core::account::AccountId;
use stablecoin_core::{
    compute_position_pda, compute_position_vault_pda, compute_protocol_parameters_pda,
    compute_redemption_price_state_pda, compute_stability_fee_accumulator_pda,
    compute_stablecoin_definition_pda, compute_stablecoin_master_holding_pda,
};

fn parse_account_id(s: &str, what: &str) -> AccountId {
    AccountId::from_str(s).unwrap_or_else(|_| panic!("{what} must be a base58 account id"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [stablecoin_s, mode] if mode == "globals" => {
            print_globals(parse_account_id(stablecoin_s, "stablecoin program account id"));
        }
        [stablecoin_s, owner_s, nonce_s] => {
            let stablecoin = parse_account_id(stablecoin_s, "stablecoin program account id");
            let owner = parse_account_id(owner_s, "owner");
            let position_nonce: u64 = nonce_s.parse().expect("position_nonce must be a u64");
            print_position(stablecoin, owner, position_nonce);
        }
        _ => {
            eprintln!(
                "usage: stablecoin_pdas <stablecoin_account_id> globals\n       stablecoin_pdas \
                 <stablecoin_account_id> <owner> <position_nonce>"
            );
            std::process::exit(1);
        }
    }
}

fn print_globals(stablecoin: AccountId) {
    println!(
        "protocol_parameters        {}",
        compute_protocol_parameters_pda(stablecoin)
    );
    println!(
        "stability_fee_accumulator  {}",
        compute_stability_fee_accumulator_pda(stablecoin)
    );
    println!(
        "redemption_price_state     {}",
        compute_redemption_price_state_pda(stablecoin)
    );
    println!(
        "stablecoin_definition      {}",
        compute_stablecoin_definition_pda(stablecoin)
    );
    println!(
        "stablecoin_master_holding  {}",
        compute_stablecoin_master_holding_pda(stablecoin)
    );
}

fn print_position(stablecoin: AccountId, owner: AccountId, position_nonce: u64) {
    let position = compute_position_pda(stablecoin, owner, position_nonce);
    println!("position        {position}");
    println!(
        "position_vault  {}",
        compute_position_vault_pda(stablecoin, position)
    );
}
