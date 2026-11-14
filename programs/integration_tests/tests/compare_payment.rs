#[path = "support/receipt_fixture.rs"]
mod fixture;
use fixture::*;
use integration_tests::{assert_refused, submit};
use token_comparison_example::receipts::{Command, Payment, Phase, Request};
fn request(reject: bool) -> Request {
    Request::Payment(Payment {
        source: id(2),
        amount: 100,
        pay: 60,
        burn: 10,
        reject_at_completion: reject,
    })
}

#[test]
fn funded_receipt_executes_without_the_payers_credential() {
    let mut chain = Chain::new(false);
    chain.fund(0, request(false), true).expect("fund");
    assert_eq!((chain.balance(id(2)), chain.balance(escrow(0))), (900, 100));
    assert_eq!(chain.receipt_state().phase, Phase::Funded);
    chain.execute(0).expect("execute");
    assert_eq!(
        (
            chain.balance(id(2)),
            chain.balance(id(3)),
            chain.balance(escrow(0)),
            chain.supply()
        ),
        (930, 60, 0, 990)
    );
    assert_eq!(chain.receipt_state().completed, 1);
}
#[test]
fn failed_execution_preserves_funding_and_owner_can_recover() {
    let mut chain = Chain::new(false);
    chain.fund(0, request(true), true).expect("fund");
    let before = chain.snapshot(0);
    assert_refused(&chain.execute(0), "example rejects after receipt effects");
    assert_eq!(chain.snapshot(0), before);
    assert_eq!((chain.balance(id(2)), chain.balance(escrow(0))), (900, 100));
    chain.cancel(0, 2, true).expect("refund");
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
fn cancellation_requires_the_exact_receipt_owner() {
    let mut chain = Chain::new(false);
    chain.fund(0, request(false), true).expect("fund");
    let before = chain.snapshot(0);
    assert_refused(
        &chain.cancel(0, 2, false),
        "receipt owner must authorize cancellation",
    );
    assert_refused(&chain.cancel(0, 3, true), "wrong receipt owner");
    assert_eq!(chain.snapshot(0), before);
}
#[test]
fn consumed_or_cancelled_ids_cannot_be_reused() {
    let mut chain = Chain::new(false);
    chain.fund(0, request(false), true).expect("fund");
    chain.execute(0).expect("execute");
    assert_refused(&chain.execute(0), "no funded receipt");
    assert_refused(
        &chain.fund(0, request(false), true),
        "receipt id already used or out of order",
    );
    chain.fund(1, request(false), true).expect("next escrow");
    chain.cancel(1, 2, true).expect("cancel");
    assert_refused(
        &chain.fund(1, request(false), true),
        "receipt id already used or out of order",
    );
    assert_eq!((chain.balance(escrow(0)), chain.balance(escrow(1))), (0, 0));
}
#[test]
fn funding_requires_a_real_debit_and_rejects_excessive_budget() {
    let mut chain = Chain::new(false);
    let before = chain.snapshot(0);
    assert_refused(
        &chain.fund(0, request(false), false),
        "sender holding must authorize the transfer",
    );
    let Request::Payment(mut op) = request(false) else {
        panic!("payment")
    };
    op.pay = 101;
    assert_refused(
        &chain.fund(0, Request::Payment(op), true),
        "distribution exceeds received amount",
    );
    assert_eq!(chain.snapshot(0), before);
}
#[test]
fn missing_recipient_keeps_receipt_funded_for_retry() {
    let mut chain = Chain::new(false);
    chain.fund(0, request(false), true).expect("fund");
    let before = chain.snapshot(0);
    let actors: Vec<_> = chain
        .actors(0)
        .into_iter()
        .filter(|a| *a != token(id(3)))
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
    chain.execute(0).expect("retry");
}
#[test]
fn zero_receipt_is_still_consumed_once() {
    let mut chain = Chain::new(false);
    chain
        .fund(
            0,
            Request::Payment(Payment {
                source: id(2),
                amount: 0,
                pay: 0,
                burn: 0,
                reject_at_completion: false,
            }),
            true,
        )
        .expect("zero fund");
    chain.execute(0).expect("zero execute");
    assert_eq!(chain.balance(id(2)), 1_000);
    assert_refused(&chain.execute(0), "no funded receipt");
}

#[test]
fn expiry_returns_funds_without_an_owner_credential() {
    let mut chain = Chain::new(false);
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
    let mut chain = Chain::new(false);
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
