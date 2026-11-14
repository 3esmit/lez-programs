#[path = "support/receipt_fixture.rs"]
mod fixture;
use fixture::*;
use integration_tests::{assert_refused, key, submit};
use token_comparison_example::receipts::{Command, Phase, Redeem, Request};
use token_core::Message;
fn request(reject: bool) -> Request {
    Request::Redeem(Redeem {
        source: id(2),
        shares: 100,
        recipient_a: id(21),
        recipient_b: id(22),
        min_a: 100,
        min_b: 200,
        reject_at_completion: reject,
    })
}

#[test]
fn funded_shares_are_redeemed_without_owner_credential() {
    let mut chain = Chain::new(true);
    chain.fund(0, request(false), true).expect("fund");
    assert_eq!(
        (
            chain.balance(id(2)),
            chain.balance(escrow(0)),
            chain.supply()
        ),
        (900, 100, 1_000)
    );
    chain.execute(0).expect("redeem");
    assert_eq!(
        (chain.balance(id(21)), chain.balance(id(22)), chain.supply()),
        (100, 200, 900)
    );
    assert_eq!(
        (
            chain.balance(vault(11)),
            chain.balance(vault(13)),
            chain.balance(escrow(0))
        ),
        (900, 1_800, 0)
    );
    assert_eq!(chain.receipt_state().phase, Phase::Ready);
}
#[test]
fn donation_after_funding_changes_execution_quote() {
    let mut chain = Chain::new(true);
    chain.fund(0, request(false), true).expect("fund");
    chain.donate(11, 12, 500);
    chain.execute(0).expect("current reserves");
    assert_eq!((chain.balance(id(21)), chain.balance(id(22))), (150, 200));
}
#[test]
fn outside_share_burn_after_funding_changes_supply() {
    let mut chain = Chain::new(true);
    chain.fund(0, request(false), true).expect("fund");
    submit(
        &mut chain.state,
        token(id(2)),
        &[token(id(2)), token(id(15))],
        &[&key(2)],
        &Message::Burn { amount: 100 },
    )
    .expect("external burn");
    chain.execute(0).expect("current supply");
    assert_eq!(
        (chain.balance(id(21)), chain.balance(id(22)), chain.supply()),
        (111, 222, 800)
    );
}
#[test]
fn late_failure_preserves_prior_funding_until_owner_recovers() {
    let mut chain = Chain::new(true);
    chain.fund(0, request(true), true).expect("fund");
    let before = chain.snapshot(0);
    assert_refused(&chain.execute(0), "example rejects after receipt effects");
    assert_eq!(chain.snapshot(0), before);
    assert_eq!(
        (
            chain.balance(id(2)),
            chain.balance(escrow(0)),
            chain.supply()
        ),
        (900, 100, 1_000)
    );
    chain.cancel(0, 2, true).expect("recover");
    assert_eq!((chain.balance(id(2)), chain.balance(escrow(0))), (1_000, 0));
}
#[test]
fn minimum_failure_leaves_a_recoverable_receipt() {
    let mut chain = Chain::new(true);
    let Request::Redeem(mut op) = request(false) else {
        panic!("redeem")
    };
    op.min_a = 101;
    chain.fund(0, Request::Redeem(op), true).expect("fund");
    let before = chain.snapshot(0);
    assert_refused(&chain.execute(0), "redemption below requested minimum");
    assert_eq!(chain.snapshot(0), before);
    chain.cancel(0, 2, true).expect("refund");
    assert_eq!(chain.balance(id(2)), 1_000);
}
#[test]
fn receipt_id_and_owner_are_bound_to_the_request() {
    let mut chain = Chain::new(true);
    chain.fund(0, request(false), true).expect("fund");
    let before = chain.snapshot(0);
    assert_refused(&chain.execute(1), "wrong receipt id");
    assert_refused(
        &chain.cancel(0, 2, false),
        "receipt owner must authorize cancellation",
    );
    assert_refused(&chain.cancel(0, 21, true), "wrong receipt owner");
    assert_eq!(chain.snapshot(0), before);
    chain.execute(0).expect("execute");
    assert_refused(&chain.execute(0), "no funded receipt");
    assert_refused(
        &chain.fund(0, request(false), true),
        "receipt id already used or out of order",
    );
}
#[test]
fn wrong_asset_and_missing_recipient_cannot_settle() {
    let mut chain = Chain::new(true);
    let Request::Redeem(mut op) = request(false) else {
        panic!("redeem")
    };
    op.source = id(12);
    assert_refused(
        &chain.fund(0, Request::Redeem(op), true),
        "unexpected token definition",
    );
    chain.fund(0, request(false), true).expect("fund");
    let before = chain.snapshot(0);
    let actors: Vec<_> = chain
        .actors(0)
        .into_iter()
        .filter(|a| *a != token(id(22)))
        .collect();
    assert!(submit(
        &mut chain.state,
        processor(),
        &actors,
        &[],
        &Command::Execute { id: 0 }
    )
    .is_err());
    assert_eq!(chain.snapshot(0), before);
}
#[test]
fn all_shares_can_be_funded_and_redeemed() {
    let mut chain = Chain::new(true);
    let Request::Redeem(mut op) = request(false) else {
        panic!("redeem")
    };
    op.shares = 1_000;
    op.min_a = 1_000;
    op.min_b = 2_000;
    chain.fund(0, Request::Redeem(op), true).expect("fund all");
    chain.execute(0).expect("redeem all");
    assert_eq!(
        (chain.balance(id(21)), chain.balance(id(22)), chain.supply()),
        (1_000, 2_000, 0)
    );
    assert_eq!((chain.balance(vault(11)), chain.balance(vault(13))), (0, 0));
}

#[test]
fn expiry_returns_funds_without_an_owner_credential() {
    let mut chain = Chain::new(true);
    chain.fund(0, request(false), true).expect("fund");
    let before = chain.snapshot(0);
    assert!(submit_at(&mut chain, 0, &Command::Expire { id: 0 }, 999).is_err());
    assert_eq!(chain.snapshot(0), before);
    assert!(submit_at(&mut chain, 0, &Command::Execute { id: 0 }, 1_000).is_err());
    assert_eq!(chain.snapshot(0), before);
    submit_at(&mut chain, 0, &Command::Expire { id: 0 }, 1_000)
        .expect("permissionless expiry refund");
    assert_eq!(
        (
            chain.balance(id(2)),
            chain.balance(escrow(0)),
            chain.supply()
        ),
        (1_000, 0, 1_000)
    );
    assert_eq!(chain.receipt_state().phase, Phase::Ready);
}

#[test]
fn funding_cannot_reserve_the_processor_beyond_the_lifetime_bound() {
    use token_comparison_example::receipts::MAX_RECEIPT_LIFETIME_MILLIS;
    let mut chain = Chain::new(true);
    let before = chain.snapshot(0);
    assert!(chain
        .fund_before(
            0,
            request(false),
            true,
            MAX_RECEIPT_LIFETIME_MILLIS
                .checked_add(1)
                .expect("test deadline")
        )
        .is_err());
    assert_eq!(chain.snapshot(0), before);
    chain
        .fund_before(0, request(false), true, MAX_RECEIPT_LIFETIME_MILLIS)
        .expect("maximum lifetime");
    assert_eq!(chain.receipt_state().phase, Phase::Funded);
}
