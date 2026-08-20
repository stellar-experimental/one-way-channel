#![cfg(test)]

extern crate std;

use std::vec;
use std::vec::Vec as StdVec;

use ed25519_dalek::SigningKey;
use soroban_sdk::{
    testutils::{Address as _, Events as _},
    xdr, Address, Env,
};

use crate::testutils::*;
use crate::{merkle, Error, Pair, Receipt, RowAbsence, Send, ShardTip};

fn has_event_type(env: &Env, contract: &Address, event_name: &str) -> bool {
    let events = env.events().all().filter_by_contract(contract);
    let target = xdr::ScVal::Symbol(xdr::ScSymbol(event_name.try_into().unwrap()));
    events.events().iter().any(|e| match &e.body {
        xdr::ContractEventBody::V0(body) => body.topics.first() == Some(&target),
    })
}

/// A deployed clearing system with the blog post's four accounts registered
/// and finalized: (a, b, c, d) open with balances (100, 40, 25, 35).
struct World {
    h: Harness,
    eng: Engine,
    funder: Address,
    a: SigningKey,
    b: SigningKey,
    c: SigningKey,
    d: SigningKey,
}

fn world() -> World {
    let h = Harness::new();
    let funder = Address::generate(&h.env);
    h.mint(&funder, 100_000);

    let a = SigningKey::from_bytes(&[1u8; 32]);
    let b = SigningKey::from_bytes(&[2u8; 32]);
    let c = SigningKey::from_bytes(&[3u8; 32]);
    let d = SigningKey::from_bytes(&[4u8; 32]);

    let mut eng = Engine::new(&h);
    for (key, amount) in [(&a, 100i128), (&b, 40), (&c, 25), (&d, 35)] {
        eng.deposit(&h, &funder, &key_of(&h.env, key), amount);
    }
    eng.begin_epoch();
    let close = eng.build_close(&h);
    close.submit(&h);
    h.pass(WINDOW + 1);
    h.client().finalize();

    World { h, eng, funder, a, b, c, d }
}

/// Serves the blog post's six payments in the current epoch:
/// a -20-> b, b -12-> c, c -7-> d, d -5-> a, c -4-> b, d -6-> b,
/// with b's three incoming payments each assigned to its own shard.
fn blog_payments(w: &mut World) -> StdVec<Pair> {
    w.eng.begin_epoch();
    let p1 = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let p2 = w.eng.pay(&w.h, 1, 2, 12, 0, &w.b);
    let p3 = w.eng.pay(&w.h, 2, 3, 7, 0, &w.c);
    let p4 = w.eng.pay(&w.h, 3, 0, 5, 0, &w.d);
    let p5 = w.eng.pay(&w.h, 2, 1, 4, 1, &w.c);
    let p6 = w.eng.pay(&w.h, 3, 1, 6, 2, &w.d);
    vec![p1, p2, p3, p4, p5, p6]
}

/// Registration via deposits: the first close consumes the chain-sealed
/// deposit records, registers the keys, and finalizes with the deposited
/// liability.
#[test]
fn test_register_and_close() {
    let w = world();
    let client = w.h.client();

    assert_eq!(client.finalized_epoch(), 1);
    assert_eq!(client.next_epoch(), 1);
    assert_eq!(client.finalized_liability(), 200);
    assert_eq!(client.custody(), 200);
    assert_eq!(client.finalized_root(), state_root(&w.h.env, &w.eng.history[1]));

    // The close consumed all four deposit records.
    let slot = client.slot(&0).unwrap();
    assert_eq!(slot.header.deposits_to, 4);
    assert_eq!(slot.header.deposits, 200);
    assert_eq!(slot.header.rows, 4);

    // The registered leaves carry the deposited balances.
    assert_eq!(w.eng.history[1][0].balance, 100);
    assert_eq!(w.eng.history[1][1].balance, 40);
    assert_eq!(w.eng.history[1][2].balance, 25);
    assert_eq!(w.eng.history[1][3].balance, 35);
}

/// The blog post's example epoch: six payments net to closing balances
/// (85, 58, 26, 31), gross debit equals gross credit at 54, and b's three
/// receive shards end at tips (20,1), (4,1), (6,1) summing to credit 30
/// across 3 receipts.
#[test]
fn test_blog_example_payments() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);

    // One row per changed account: not the six payments, but the four
    // accounts they changed.
    assert_eq!(close.header.rows, 4);
    assert_eq!(close.header.debits, 54);
    assert_eq!(close.header.credits, 54);
    assert_eq!(close.header.deposits, 0);
    assert_eq!(close.header.withdrawals, 0);

    // Exact closing balances.
    assert_eq!(close.closing[0].balance, 85);
    assert_eq!(close.closing[1].balance, 58);
    assert_eq!(close.closing[2].balance, 26);
    assert_eq!(close.closing[3].balance, 31);

    // b's shard tips: (h_b, G_b, J_b) = (3, 30, 3).
    let b_tips = &close.shard_tips[close.position(1).unwrap()];
    assert_eq!(b_tips.len(), 3);
    assert_eq!(b_tips[0], ShardTip { count: 1, credit: 20 });
    assert_eq!(b_tips[1], ShardTip { count: 1, credit: 4 });
    assert_eq!(b_tips[2], ShardTip { count: 1, credit: 6 });
    assert_eq!(close.closing[1].credit, 30);
    assert_eq!(close.closing[1].receipts, 3);

    close.submit(&w.h);
    w.h.pass(WINDOW + 1);
    w.h.client().finalize();

    // Payments cancel in the liability: only boundary flows move it.
    assert_eq!(w.h.client().finalized_liability(), 200);
    assert_eq!(w.h.client().finalized_root(), state_root(&w.h.env, &w.eng.history[2]));
}

/// Epoch transitions are asynchronous: spending continues in epoch e+1 on
/// the preserved head while epoch e's close is still pending, and both
/// closes finalize in order. Reproduces the blog post's rollover figures:
/// a's preserved head is 80, the in-flight credit is 5, and spending 20 and
/// 15 in the successor leaves 85 - 20 - 15 = 50.
#[test]
fn test_rollover_preserved_head() {
    let mut w = world();
    blog_payments(&mut w);
    let close1 = w.eng.build_close(&w.h);
    close1.submit(&w.h);

    // Epoch 2 spends against the rolled-over state while epoch 1 is
    // pending.
    w.eng.begin_epoch();
    w.eng.pay(&w.h, 0, 2, 20, 0, &w.a);
    w.eng.pay(&w.h, 0, 2, 15, 0, &w.a);
    let close2 = w.eng.build_close(&w.h);
    assert_eq!(close2.closing[0].balance, 50);
    close2.submit(&w.h);

    let client = w.h.client();
    assert_eq!(client.next_epoch(), 3);
    assert_eq!(client.finalized_epoch(), 1);

    // Both windows pass; closes finalize strictly in order.
    w.h.pass(WINDOW + 1);
    client.finalize();
    assert_eq!(client.finalized_epoch(), 2);
    client.finalize();
    assert_eq!(client.finalized_epoch(), 3);
    assert_eq!(client.finalized_liability(), 200);
    assert_eq!(client.finalized_root(), state_root(&w.h.env, &w.eng.history[3]));
}

/// A close with no rows carries zero totals and leaves the state root
/// unchanged.
#[test]
fn test_empty_close() {
    let mut w = world();
    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    assert_eq!(close.header.rows, 0);
    assert_eq!(close.header.state_root, close.header.state_root_after);
    close.submit(&w.h);
    w.h.pass(WINDOW + 1);
    w.h.client().finalize();
    assert_eq!(w.h.client().finalized_epoch(), 2);
}

/// A deposit for an already-registered key tops up its account at the next
/// close.
#[test]
fn test_deposit_top_up() {
    let mut w = world();
    w.eng.deposit(&w.h, &w.funder, &key_of(&w.h.env, &w.a), 60);
    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    assert_eq!(close.header.deposits, 60);
    assert_eq!(close.closing[0].balance, 160);
    close.submit(&w.h);
    w.h.pass(WINDOW + 1);
    w.h.client().finalize();
    assert_eq!(w.h.client().finalized_liability(), 260);
    assert_eq!(w.h.client().custody(), 260);
}

/// A queued exit is consumed by the next close and released to its signed
/// destination once that close finalizes.
#[test]
fn test_exit_and_release() {
    let mut w = world();
    let destination = Address::generate(&w.h.env);
    let deadline = w.h.now() + EXIT_DELAY + 200;
    let sequence = w.eng.exit(&w.h, 0, &w.a, &destination, 30, false, deadline);

    let client = w.h.client();
    assert_eq!(client.pending_exits(&key_of(&w.h.env, &w.a)), 30);

    // Releasing before the consuming close finalizes fails.
    assert_eq!(client.try_release(&sequence).err(), Some(Ok(Error::NotReleasable)));

    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    assert_eq!(close.header.withdrawals, 30);
    assert_eq!(close.header.withdrawal_records, 1);
    assert_eq!(close.closing[0].balance, 70);
    close.submit(&w.h);
    w.h.pass(WINDOW + 1);
    client.finalize();

    client.release(&sequence);
    assert_eq!(w.h.token_client().balance(&destination), 30);
    assert_eq!(client.custody(), 170);
    assert_eq!(client.finalized_liability(), 170);
    assert_eq!(client.pending_exits(&key_of(&w.h.env, &w.a)), 0);
    assert_eq!(client.try_release(&sequence).err(), Some(Ok(Error::AlreadyPaid)));
}

/// Queued exits reserve balance: a second exit that would overdraw the
/// account against the finalized root is rejected.
#[test]
fn test_exit_reserves_balance() {
    let mut w = world();
    let destination = Address::generate(&w.h.env);
    let deadline = w.h.now() + EXIT_DELAY + 200;

    // a's finalized balance is 100.
    w.eng.exit(&w.h, 0, &w.a, &destination, 80, false, deadline);
    let registry = w.eng.history[1].clone();
    let root = state_root(&w.h.env, &registry);
    assert_eq!(root, w.h.client().finalized_root());
    let body = crate::Exit::new(w.h.id.clone(), destination.clone(), 30, false, deadline, root);
    let sig = sign(&w.h.env, &w.a, &body.bytes());
    let proof = state_proof(&w.h.env, &registry, 0);
    let result = w.h.client().try_exit(&destination, &30, &false, &deadline, &sig, &0, &registry[0], &proof);
    assert_eq!(result.err(), Some(Ok(Error::InsufficientBalance)));
}

/// An exit deadline closer than the minimum delay is rejected.
#[test]
fn test_exit_deadline_too_soon() {
    let w = world();
    let destination = Address::generate(&w.h.env);
    let registry = w.eng.history[1].clone();
    let root = state_root(&w.h.env, &registry);
    let deadline = w.h.now() + EXIT_DELAY - 1;
    let body = crate::Exit::new(w.h.id.clone(), destination.clone(), 10, false, deadline, root);
    let sig = sign(&w.h.env, &w.a, &body.bytes());
    let proof = state_proof(&w.h.env, &registry, 0);
    let result = w.h.client().try_exit(&destination, &10, &false, &deadline, &sig, &0, &registry[0], &proof);
    assert_eq!(result.err(), Some(Ok(Error::DeadlineTooSoon)));
}

/// An exit signed by the wrong key fails signature verification.
#[test]
fn test_exit_wrong_signer() {
    let w = world();
    let destination = Address::generate(&w.h.env);
    let registry = w.eng.history[1].clone();
    let root = state_root(&w.h.env, &registry);
    let deadline = w.h.now() + EXIT_DELAY + 10;
    let body = crate::Exit::new(w.h.id.clone(), destination.clone(), 10, false, deadline, root);
    let sig = sign(&w.h.env, &w.b, &body.bytes());
    let proof = state_proof(&w.h.env, &registry, 0);
    assert!(w.h.client().try_exit(&destination, &10, &false, &deadline, &sig, &0, &registry[0], &proof).is_err());
}

/// A certificate below the quorum is rejected.
#[test]
fn test_submit_insufficient_quorum() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);
    let cert = w.h.cert_of(&close.header, &[0, 1]);
    let result = w.h.client().try_submit(&close.header, &cert, &close.terminal(), &close.terminal_proof(&w.h.env));
    assert_eq!(result.err(), Some(Ok(Error::QuorumNotMet)));
}

/// Certificate signatures must be in strictly increasing committee index
/// order: one validator cannot be counted twice.
#[test]
fn test_submit_duplicate_validator() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);
    let cert = w.h.cert_of(&close.header, &[0, 1, 1]);
    let result = w.h.client().try_submit(&close.header, &cert, &close.terminal(), &close.terminal_proof(&w.h.env));
    assert_eq!(result.err(), Some(Ok(Error::InvalidCertificate)));
}

/// A signature by a key other than the named validator's fails.
#[test]
fn test_submit_bad_validator_signature() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);
    let mut cert = w.h.cert_of(&close.header, &[0, 1]);
    // Validator 2's slot signed by validator 3's key.
    let forged = sign(&w.h.env, &w.h.validators[3], &close.header.bytes());
    cert.push_back(crate::Signature { index: 2, signature: forged });
    assert!(w.h.client().try_submit(&close.header, &cert, &close.terminal(), &close.terminal_proof(&w.h.env)).is_err());
}

/// A close must chain from the previous close's state root.
#[test]
fn test_submit_wrong_parent_root() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);
    let mut header = close.header.clone();
    header.state_root = merkle::empty(&w.h.env);
    let cert = w.h.cert(&header);
    let result = w.h.client().try_submit(&header, &cert, &close.terminal(), &close.terminal_proof(&w.h.env));
    assert_eq!(result.err(), Some(Ok(Error::WrongParentRoot)));
}

/// A close for the wrong epoch is rejected.
#[test]
fn test_submit_wrong_epoch() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);
    let mut header = close.header.clone();
    header.epoch = 5;
    let cert = w.h.cert(&header);
    let result = w.h.client().try_submit(&header, &cert, &close.terminal(), &close.terminal_proof(&w.h.env));
    assert_eq!(result.err(), Some(Ok(Error::WrongEpoch)));
}

/// The terminal row's prefix must equal the header's totals, even with a
/// valid certificate.
#[test]
fn test_submit_totals_mismatch() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);
    let mut header = close.header.clone();
    header.debits = 53;
    header.credits = 53;
    let cert = w.h.cert(&header);
    let result = w.h.client().try_submit(&header, &cert, &close.terminal(), &close.terminal_proof(&w.h.env));
    assert_eq!(result.err(), Some(Ok(Error::TotalsMismatch)));
}

/// Gross debit must equal gross credit: a close whose terminal prefix does
/// not conserve payments is rejected.
#[test]
fn test_submit_payments_not_conserved() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);

    // Rebuild the close with a tampered terminal prefix crediting one unit
    // out of thin air, and a header matching it.
    let mut rows = close.rows.clone();
    let last = rows.len() - 1;
    rows[last].prefix.credits += 1;
    let mut header = close.header.clone();
    header.credits += 1;
    header.change_root = change_root(&w.h.env, &rows);
    let cert = w.h.cert(&header);
    let result = w.h.client().try_submit(&header, &cert, &Some(rows[last].clone()), &row_proof(&w.h.env, &rows, last));
    assert_eq!(result.err(), Some(Ok(Error::PaymentsNotConserved)));
}

/// The consumed deposit total must reproduce the chain-sealed boundary.
#[test]
fn test_submit_boundary_mismatch() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);

    // Claim a deposit total the records do not carry, consistently across
    // the terminal prefix and the header.
    let mut rows = close.rows.clone();
    let last = rows.len() - 1;
    rows[last].prefix.deposits += 10;
    rows[last].prefix.credits += 10;
    let mut header = close.header.clone();
    header.deposits += 10;
    header.credits += 10;
    header.debits += 10;
    rows[last].prefix.debits += 10;
    header.change_root = change_root(&w.h.env, &rows);
    let cert = w.h.cert(&header);
    let result = w.h.client().try_submit(&header, &cert, &Some(rows[last].clone()), &row_proof(&w.h.env, &rows, last));
    assert_eq!(result.err(), Some(Ok(Error::BoundaryMismatch)));
}

/// Finalizing before the challenge window has passed fails.
#[test]
fn test_finalize_before_window() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);
    w.h.pass(WINDOW); // The deadline itself is still challengeable.
    assert_eq!(w.h.client().try_finalize().err(), Some(Ok(Error::ChallengeWindowOpen)));
    w.h.pass(1);
    w.h.client().finalize();
}

/// Challenge 1a: the operator omits an accepted payment from the close. The
/// payer's retained pair carries a debit above the public debit marker, the
/// close falls, and a corrected close for the same epoch settles.
#[test]
fn test_challenge_debit_omitted_payment() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();

    // The operator accepts both payments and returns receipts...
    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    w.eng.pay(&w.h, 2, 3, 7, 0, &w.c);
    let honest = w.eng.build_close(&w.h);

    // ...but the close it publishes omits a's payment entirely. The fork's
    // corpus is internally consistent and passes committee validation:
    // only the retained receipt proves the fault.
    fork.pay(&w.h, 2, 3, 7, 0, &w.c);
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);

    let client = w.h.client();
    let proof = state_proof(&w.h.env, &fraud.closing, 0);
    client.challenge_debit(&1, &pair, &0, &fraud.closing[0], &proof);
    assert!(has_event_type(&w.h.env, &w.h.id, "invalidate"));

    // The queue was truncated; the corrected close settles the epoch.
    assert_eq!(client.next_epoch(), 1);
    assert_eq!(client.slot(&1), None);
    honest.submit(&w.h);
    w.h.pass(WINDOW + 1);
    client.finalize();
    assert_eq!(client.finalized_root(), state_root(&w.h.env, &w.eng.history[2]));
}

/// A pair the close accounts for is no contradiction.
#[test]
fn test_challenge_debit_no_contradiction() {
    let mut w = world();
    w.eng.begin_epoch();
    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);

    let proof = state_proof(&w.h.env, &close.closing, 0);
    let result = w.h.client().try_challenge_debit(&1, &pair, &0, &close.closing[0], &proof);
    assert_eq!(result.err(), Some(Ok(Error::NoContradiction)));

    // The close survives and finalizes.
    w.h.pass(WINDOW + 1);
    w.h.client().finalize();
}

/// Challenge 1b: a byzantine payer double-signs two sends at one debit and
/// the operator acknowledges both. The close can carry only one terminal
/// pair; the other retained pair contradicts it.
#[test]
fn test_challenge_debit_body_equivocation() {
    let mut w = world();
    w.eng.begin_epoch();

    // The close carries a's payment to b as the terminal pair.
    w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let close = w.eng.build_close(&w.h);

    // a also signed a send to c at the same debit, and the operator
    // acknowledged it privately.
    let send = Send::new(w.h.id.clone(), key_of(&w.h.env, &w.a), key_of(&w.h.env, &w.c), 20, 20, 1);
    let send_sig = sign(&w.h.env, &w.a, &send.bytes());
    let receipt = Receipt::new(w.h.id.clone(), key_of(&w.h.env, &w.c), 0, 20, send.txid(), 20, 1, 1);
    let receipt_sig = w.h.sign_receipt(&receipt);
    let equivocation = Pair { receipt, receipt_sig, send, send_sig };

    close.submit(&w.h);
    let position = close.position(0).unwrap();
    let proof = row_proof(&w.h.env, &close.rows, position);
    w.h.client().challenge_debit_body(&1, &equivocation, &close.rows[position], &(position as u32), &proof);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// The terminal pair itself is no contradiction at its own debit.
#[test]
fn test_challenge_debit_body_no_contradiction() {
    let mut w = world();
    w.eng.begin_epoch();
    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);

    let position = close.position(0).unwrap();
    let proof = row_proof(&w.h.env, &close.rows, position);
    let result = w.h.client().try_challenge_debit_body(&1, &pair, &close.rows[position], &(position as u32), &proof);
    assert_eq!(result.err(), Some(Ok(Error::NoContradiction)));
}

/// Challenge 2a: the close understates a receive-shard tip. The recipient's
/// retained receipt at a strictly higher tip contradicts the authenticated
/// public tip.
#[test]
fn test_challenge_tip_understated() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();

    w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let pair = w.eng.pay(&w.h, 2, 1, 4, 0, &w.c);

    // The published close drops the second payment to b's shard 0.
    fork.pay(&w.h, 0, 1, 20, 0, &w.a);
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);

    let position = fraud.position(1).unwrap();
    let tips = &fraud.shard_tips[position];
    assert_eq!(tips[0], ShardTip { count: 1, credit: 20 });
    w.h.client().challenge_tip(
        &1,
        &pair,
        &fraud.rows[position],
        &(position as u32),
        &row_proof(&w.h.env, &fraud.rows, position),
        &(tips.len() as u32),
        &tips[0],
        &tip_proof(&w.h.env, tips, 0),
    );
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// An honest tip is no contradiction.
#[test]
fn test_challenge_tip_no_contradiction() {
    let mut w = world();
    w.eng.begin_epoch();
    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);

    let position = close.position(1).unwrap();
    let tips = &close.shard_tips[position];
    let result = w.h.client().try_challenge_tip(
        &1,
        &pair,
        &close.rows[position],
        &(position as u32),
        &row_proof(&w.h.env, &close.rows, position),
        &(tips.len() as u32),
        &tips[0],
        &tip_proof(&w.h.env, tips, 0),
    );
    assert_eq!(result.err(), Some(Ok(Error::NoContradiction)));
}

/// Challenge 2b: the close binds fewer shards than the operator advanced.
/// A retained receipt on a shard at or beyond the bound count contradicts
/// the authenticated absence tip (0, 0).
#[test]
fn test_challenge_tip_absent_shard() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();

    w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let pair = w.eng.pay(&w.h, 2, 1, 4, 1, &w.c);

    // The published close carries only shard 0.
    fork.pay(&w.h, 0, 1, 20, 0, &w.a);
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);

    let position = fraud.position(1).unwrap();
    let tips = &fraud.shard_tips[position];
    assert_eq!(tips.len(), 1);
    w.h.client().challenge_tip_absent(
        &1,
        &pair,
        &fraud.rows[position],
        &(position as u32),
        &row_proof(&w.h.env, &fraud.rows, position),
        &1,
        &credit_subroot(&w.h.env, tips),
    );
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Challenge 2c: the close omits the recipient's row entirely. Row absence
/// is proven by the adjacent rows straddling the account index, and the
/// authenticated tip (0, 0) contradicts any retained receipt.
#[test]
fn test_challenge_tip_no_row_between() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();

    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    w.eng.pay(&w.h, 0, 2, 10, 0, &w.a);

    // The published close keeps only a's payment to c: rows for accounts 0
    // and 2 straddle the omitted account 1.
    fork.pay(&w.h, 0, 2, 10, 0, &w.a);
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);
    assert_eq!(fraud.position(1), None);

    let absence = RowAbsence::Between(
        0,
        fraud.rows[0].clone(),
        row_proof(&w.h.env, &fraud.rows, 0),
        fraud.rows[1].clone(),
        row_proof(&w.h.env, &fraud.rows, 1),
    );
    let proof = state_proof(&w.h.env, &fraud.closing, 1);
    w.h.client().challenge_tip_no_row(&1, &pair, &1, &fraud.closing[1], &proof, &absence);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Challenge 2c with an empty close: the operator publishes no rows at all
/// despite having acknowledged a payment.
#[test]
fn test_challenge_tip_no_row_empty_close() {
    let mut w = world();
    w.eng.begin_epoch();
    let fork = w.eng.clone();

    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);

    let mut fork = fork;
    let fraud = fork.build_close(&w.h);
    assert_eq!(fraud.header.rows, 0);
    fraud.submit(&w.h);

    let proof = state_proof(&w.h.env, &fraud.closing, 1);
    w.h.client().challenge_tip_no_row(&1, &pair, &1, &fraud.closing[1], &proof, &RowAbsence::Empty);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Row absence at the edges of the sorted rows: Before proves absence below
/// the first row's account.
#[test]
fn test_challenge_tip_no_row_before() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();

    let pair = w.eng.pay(&w.h, 2, 0, 5, 0, &w.c);
    w.eng.pay(&w.h, 2, 3, 7, 0, &w.c);

    // The published close keeps only c's payment to d: rows for accounts 2
    // and 3, and the omitted recipient is account 0, below the first row.
    fork.pay(&w.h, 2, 3, 7, 0, &w.c);
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);

    let absence = RowAbsence::Before(fraud.rows[0].clone(), row_proof(&w.h.env, &fraud.rows, 0));
    let proof = state_proof(&w.h.env, &fraud.closing, 0);
    w.h.client().challenge_tip_no_row(&1, &pair, &0, &fraud.closing[0], &proof, &absence);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Row absence at the edges of the sorted rows: After proves absence above
/// the last row's account.
#[test]
fn test_challenge_tip_no_row_after() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();

    let pair = w.eng.pay(&w.h, 0, 3, 5, 0, &w.a);
    w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);

    // The published close keeps only a's payment to b: rows for accounts 0
    // and 1, and the omitted recipient is account 3, above the last row.
    fork.pay(&w.h, 0, 1, 20, 0, &w.a);
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);

    let absence = RowAbsence::After(fraud.rows[1].clone(), row_proof(&w.h.env, &fraud.rows, 1));
    let proof = state_proof(&w.h.env, &fraud.closing, 3);
    w.h.client().challenge_tip_no_row(&1, &pair, &3, &fraud.closing[3], &proof, &absence);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Challenge 3: adjacent receipts in one shard must increase the credit by
/// exactly the upper payment.
#[test]
fn test_challenge_range_adjacent() {
    let mut w = world();
    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);

    // The operator's receipts skim 3 units between receipt 1 and receipt 2.
    let b_key = key_of(&w.h.env, &w.b);
    let send1 = Send::new(w.h.id.clone(), key_of(&w.h.env, &w.a), b_key.clone(), 10, 10, 1);
    let send1_sig = sign(&w.h.env, &w.a, &send1.bytes());
    let receipt1 = Receipt::new(w.h.id.clone(), b_key.clone(), 0, 10, send1.txid(), 10, 1, 1);
    let receipt1_sig = w.h.sign_receipt(&receipt1);
    let send2 = Send::new(w.h.id.clone(), key_of(&w.h.env, &w.a), b_key.clone(), 5, 15, 1);
    let send2_sig = sign(&w.h.env, &w.a, &send2.bytes());
    let receipt2 = Receipt::new(w.h.id.clone(), b_key.clone(), 0, 5, send2.txid(), 12, 2, 1);
    let receipt2_sig = w.h.sign_receipt(&receipt2);

    let lower = Pair {
        receipt: receipt1,
        receipt_sig: receipt1_sig,
        send: send1,
        send_sig: send1_sig,
    };
    let upper = Pair {
        receipt: receipt2,
        receipt_sig: receipt2_sig,
        send: send2,
        send_sig: send2_sig,
    };
    w.h.client().challenge_range(&1, &lower, &upper);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Challenge 3: an index gap must leave at least one base unit for each
/// omitted positive payment; a consistent gap is no contradiction.
#[test]
fn test_challenge_range_gap() {
    let mut w = world();
    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);

    let a_key = key_of(&w.h.env, &w.a);
    let b_key = key_of(&w.h.env, &w.b);
    let make = |amount: i128, debit: i128, credit: i128, count: u64| {
        let send = Send::new(w.h.id.clone(), a_key.clone(), b_key.clone(), amount, debit, 1);
        let send_sig = sign(&w.h.env, &w.a, &send.bytes());
        let receipt = Receipt::new(w.h.id.clone(), b_key.clone(), 0, amount, send.txid(), credit, count, 1);
        let receipt_sig = w.h.sign_receipt(&receipt);
        Pair { receipt, receipt_sig, send, send_sig }
    };

    let lower = make(10, 10, 10, 1);
    // Receipt 3 must carry at least 10 + 5 + 1 = 16 credit; 16 is
    // consistent, 15 is not.
    let consistent = make(5, 20, 16, 3);
    let result = w.h.client().try_challenge_range(&1, &lower, &consistent);
    assert_eq!(result.err(), Some(Ok(Error::NoContradiction)));

    let violating = make(5, 20, 15, 3);
    w.h.client().challenge_range(&1, &lower, &violating);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Challenge 4: two distinct receipt bodies reusing one receipt index
/// within a shard are a fork.
#[test]
fn test_challenge_fork_reused_index() {
    let mut w = world();
    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);

    let b_key = key_of(&w.h.env, &w.b);
    let txid1 = merkle::sha256(&w.h.env, &soroban_sdk::Bytes::from_array(&w.h.env, &[1u8]));
    let txid2 = merkle::sha256(&w.h.env, &soroban_sdk::Bytes::from_array(&w.h.env, &[2u8]));
    let first = Receipt::new(w.h.id.clone(), b_key.clone(), 0, 10, txid1, 10, 1, 1);
    let second = Receipt::new(w.h.id.clone(), b_key.clone(), 0, 12, txid2, 12, 1, 1);
    let first_sig = w.h.sign_receipt(&first);
    let second_sig = w.h.sign_receipt(&second);
    w.h.client().challenge_fork(&1, &first, &first_sig, &second, &second_sig);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Challenge 4: two distinct receipt bodies acknowledging the same payer
/// transaction differently are a fork, and identical bodies are not.
#[test]
fn test_challenge_fork_same_txid() {
    let mut w = world();
    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);

    let b_key = key_of(&w.h.env, &w.b);
    let txid = merkle::sha256(&w.h.env, &soroban_sdk::Bytes::from_array(&w.h.env, &[7u8]));
    let first = Receipt::new(w.h.id.clone(), b_key.clone(), 0, 10, txid.clone(), 10, 1, 1);
    let second = Receipt::new(w.h.id.clone(), b_key.clone(), 1, 10, txid.clone(), 10, 1, 1);
    let first_sig = w.h.sign_receipt(&first);
    let second_sig = w.h.sign_receipt(&second);

    // Identical bodies (fresh signatures) are not a fork.
    let again_sig = w.h.sign_receipt(&first);
    let result = w.h.client().try_challenge_fork(&1, &first, &first_sig, &first, &again_sig);
    assert_eq!(result.err(), Some(Ok(Error::NoContradiction)));

    w.h.client().challenge_fork(&1, &first, &first_sig, &second, &second_sig);
    assert_eq!(w.h.client().next_epoch(), 1);
}

/// Challenges are rejected after the inclusive deadline, and the close then
/// finalizes untouched.
#[test]
fn test_challenge_after_window_closed() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();
    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);

    w.h.pass(WINDOW + 1);
    let proof = state_proof(&w.h.env, &fraud.closing, 0);
    let result = w.h.client().try_challenge_debit(&1, &pair, &0, &fraud.closing[0], &proof);
    assert_eq!(result.err(), Some(Ok(Error::ChallengeWindowClosed)));
    w.h.client().finalize();
}

/// A challenge against a finalized close is rejected.
#[test]
fn test_challenge_finalized_slot() {
    let mut w = world();
    w.eng.begin_epoch();
    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);
    w.h.pass(WINDOW + 1);
    w.h.client().finalize();

    let proof = state_proof(&w.h.env, &close.closing, 0);
    let result = w.h.client().try_challenge_debit(&1, &pair, &0, &close.closing[0], &proof);
    assert_eq!(result.err(), Some(Ok(Error::NoSuchSlot)));
}

/// A successful challenge blocks the contested close and every pending
/// descendant from finalizing.
#[test]
fn test_challenge_truncates_descendants() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();
    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);

    // The operator publishes an omitting close for epoch 1 and an honest
    // close for epoch 2 on top of it.
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);
    fork.begin_epoch();
    fork.pay(&w.h, 2, 3, 7, 0, &w.c);
    let descendant = fork.build_close(&w.h);
    descendant.submit(&w.h);
    assert_eq!(w.h.client().next_epoch(), 3);

    let proof = state_proof(&w.h.env, &fraud.closing, 0);
    w.h.client().challenge_debit(&1, &pair, &0, &fraud.closing[0], &proof);
    assert_eq!(w.h.client().next_epoch(), 1);
    assert_eq!(w.h.client().slot(&1), None);
    assert_eq!(w.h.client().slot(&2), None);
}

/// A breached exit deadline freezes the system; pending closes resolve, and
/// terminal unwind pays the exit, the accounts, and the unconsumed deposit
/// until custody is empty.
#[test]
fn test_freeze_and_unwind() {
    let mut w = world();
    let client = w.h.client();

    // a queues an exit the operator never consumes, and a straggler
    // deposit arrives that no close will consume.
    let destination = Address::generate(&w.h.env);
    let deadline = w.h.now() + EXIT_DELAY;
    let sequence = w.eng.exit(&w.h, 0, &w.a, &destination, 30, false, deadline);
    let straggler = Address::generate(&w.h.env);
    w.h.mint(&straggler, 500);
    client.deposit(&straggler, &key_of(&w.h.env, &w.b), &500);
    assert_eq!(client.custody(), 700);

    // Too early to freeze.
    assert_eq!(client.try_freeze(&sequence).err(), Some(Ok(Error::DeadlineNotReached)));
    w.h.pass(EXIT_DELAY);
    client.freeze(&sequence);
    assert!(client.frozen());

    // Frozen: no new deposits, exits, or closes.
    assert_eq!(client.try_deposit(&w.funder, &key_of(&w.h.env, &w.a), &10).err(), Some(Ok(Error::Frozen)));

    // No pending closes remain, so terminal unwind opens against the
    // finalized root: the registry where (a, b, c, d) hold
    // (100, 40, 25, 35).
    let registry = w.eng.history[1].clone();

    // The queued exit pays its signed destination.
    client.unwind_exit(&sequence, &0, &registry[0], &state_proof(&w.h.env, &registry, 0));
    assert_eq!(w.h.token_client().balance(&destination), 30);
    assert_eq!(
        client.try_unwind_exit(&sequence, &0, &registry[0], &state_proof(&w.h.env, &registry, 0)).err(),
        Some(Ok(Error::AlreadyPaid))
    );

    // Each account claims its remaining balance with one Merkle proof and
    // a signed destination; a's claim nets out the unwound exit.
    let payouts: [(usize, &SigningKey, i128); 4] = [(0, &w.a, 70), (1, &w.b, 40), (2, &w.c, 25), (3, &w.d, 35)];
    for (account, signer, expected) in payouts {
        let out = Address::generate(&w.h.env);
        let sig = w.eng.claim_sig(&w.h, signer, &out);
        client.unwind_claim(&(account as u32), &registry[account], &state_proof(&w.h.env, &registry, account), &out, &sig);
        assert_eq!(w.h.token_client().balance(&out), expected);
    }

    // The straggler deposit was never consumed by a finalized close and is
    // refunded to its depositor; the four registration deposits were
    // consumed and are not.
    client.unwind_deposit(&4);
    assert_eq!(w.h.token_client().balance(&straggler), 500);
    assert_eq!(client.try_unwind_deposit(&0).err(), Some(Ok(Error::NotReleasable)));

    // Custody never leaves the chain: everything is accounted for.
    assert_eq!(client.custody(), 0);
}

/// A full-close exit drains the account's proven balance during terminal
/// unwind, and the account's residual claim nets to zero.
#[test]
fn test_full_close_exit_unwind() {
    let mut w = world();
    let client = w.h.client();
    let destination = Address::generate(&w.h.env);
    let deadline = w.h.now() + EXIT_DELAY;
    let sequence = w.eng.exit(&w.h, 3, &w.d, &destination, 10, true, deadline);

    w.h.pass(EXIT_DELAY);
    client.freeze(&sequence);

    let registry = w.eng.history[1].clone();
    client.unwind_exit(&sequence, &3, &registry[3], &state_proof(&w.h.env, &registry, 3));
    assert_eq!(w.h.token_client().balance(&destination), 35);

    let out = Address::generate(&w.h.env);
    let sig = w.eng.claim_sig(&w.h, &w.d, &out);
    client.unwind_claim(&3, &registry[3], &state_proof(&w.h.env, &registry, 3), &out, &sig);
    assert_eq!(w.h.token_client().balance(&out), 0);
}

/// An exit consumed by a finalized close cannot freeze the system: it is
/// already releasable permissionlessly.
#[test]
fn test_freeze_fails_when_releasable() {
    let mut w = world();
    let destination = Address::generate(&w.h.env);
    let deadline = w.h.now() + EXIT_DELAY + 200;
    let sequence = w.eng.exit(&w.h, 0, &w.a, &destination, 30, false, deadline);

    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);
    w.h.pass(WINDOW + 1);
    w.h.client().finalize();

    // The deadline passes with the exit unreleased, but it is covered by a
    // finalized close: anyone can release it instead.
    w.h.pass(300);
    assert_eq!(w.h.client().try_freeze(&sequence).err(), Some(Ok(Error::Releasable)));
    w.h.client().release(&sequence);
    assert_eq!(w.h.token_client().balance(&destination), 30);
}

/// Terminal unwind requires the system frozen and every pending close
/// resolved.
#[test]
fn test_unwind_requires_frozen_and_resolved() {
    let mut w = world();
    let client = w.h.client();
    let registry = w.eng.history[1].clone();

    // Not frozen.
    let result = client.try_unwind_claim(
        &0,
        &registry[0],
        &state_proof(&w.h.env, &registry, 0),
        &w.funder,
        &sign(&w.h.env, &w.a, &crate::Claim::new(w.h.id.clone(), w.funder.clone(), state_root(&w.h.env, &registry)).bytes()),
    );
    assert_eq!(result.err(), Some(Ok(Error::NotFrozen)));

    // Frozen, but a pending close remains.
    let destination = Address::generate(&w.h.env);
    let deadline = w.h.now() + EXIT_DELAY;
    let sequence = w.eng.exit(&w.h, 0, &w.a, &destination, 30, false, deadline);
    w.eng.begin_epoch();
    let close = w.eng.build_close(&w.h);
    close.submit(&w.h);
    w.h.pass(EXIT_DELAY);
    client.freeze(&sequence);
    assert_eq!(
        client.try_submit(&close.header, &w.h.cert(&close.header), &close.terminal(), &close.terminal_proof(&w.h.env)).err(),
        Some(Ok(Error::Frozen))
    );
    let result = client.try_unwind_exit(&sequence, &0, &registry[0], &state_proof(&w.h.env, &registry, 0));
    assert_eq!(result.err(), Some(Ok(Error::PendingSlotsRemain)));

    // The pending close resolves (it consumed the exit), and release pays
    // it even after the freeze.
    w.h.pass(WINDOW + 1);
    client.finalize();
    client.release(&sequence);
    assert_eq!(w.h.token_client().balance(&destination), 30);
}

/// The prepare helpers return exactly the bytes the off-chain signers sign.
#[test]
fn test_prepare_helpers() {
    let w = world();
    let client = w.h.client();
    let a_key = key_of(&w.h.env, &w.a);
    let b_key = key_of(&w.h.env, &w.b);

    let send = Send::new(w.h.id.clone(), a_key.clone(), b_key.clone(), 20, 20, 1);
    assert_eq!(client.prepare_send(&a_key, &b_key, &20, &20, &1), send.bytes());

    let receipt = Receipt::new(w.h.id.clone(), b_key.clone(), 0, 20, send.txid(), 20, 1, 1);
    assert_eq!(client.prepare_receipt(&b_key, &0, &20, &send.txid(), &20, &1, &1), receipt.bytes());

    let destination = Address::generate(&w.h.env);
    let root = client.finalized_root();
    let exit = crate::Exit::new(w.h.id.clone(), destination.clone(), 30, false, 500, root.clone());
    assert_eq!(client.prepare_exit(&destination, &30, &false, &500), exit.bytes());

    let claim = crate::Claim::new(w.h.id.clone(), destination.clone(), root);
    assert_eq!(client.prepare_claim(&destination), claim.bytes());
}

/// Constructor values are readable through the getters.
#[test]
fn test_getters() {
    let w = world();
    let client = w.h.client();
    assert_eq!(client.token(), w.h.token);
    assert_eq!(client.operator_key(), key_of(&w.h.env, &w.h.operator));
    assert_eq!(client.quorum(), QUORUM);
    assert_eq!(client.registry_depth(), DEPTH);
    assert_eq!(client.challenge_window(), WINDOW);
    assert_eq!(client.min_exit_delay(), EXIT_DELAY);
    assert_eq!(client.validators().len() as usize, VALIDATORS);
    assert!(!client.frozen());
    assert_eq!(client.deposit_count(), 4);
    assert_eq!(client.exit_count(), 0);
    assert_eq!(client.deposit_record(&0).unwrap().amount, 100);
}

/// A mismatched pair — a receipt that does not acknowledge the presented
/// send — is rejected as challenge evidence.
#[test]
fn test_challenge_pair_mismatch() {
    let mut w = world();
    w.eng.begin_epoch();
    let mut fork = w.eng.clone();
    let pair = w.eng.pay(&w.h, 0, 1, 20, 0, &w.a);
    let other = w.eng.pay(&w.h, 2, 1, 4, 0, &w.c);
    let fraud = fork.build_close(&w.h);
    fraud.submit(&w.h);

    let mismatched = Pair {
        receipt: other.receipt.clone(),
        receipt_sig: other.receipt_sig.clone(),
        send: pair.send.clone(),
        send_sig: pair.send_sig.clone(),
    };
    let proof = state_proof(&w.h.env, &fraud.closing, 0);
    let result = w.h.client().try_challenge_debit(&1, &mismatched, &0, &fraud.closing[0], &proof);
    assert_eq!(result.err(), Some(Ok(Error::PairMismatch)));
}

/// The sparse witness demonstrates the paired-root reconstruction: it
/// recomputes both state roots from the changed leaves and the shared
/// frontier alone.
#[test]
fn test_witness_reconstruction() {
    let mut w = world();
    blog_payments(&mut w);
    let close = w.eng.build_close(&w.h);
    let (root, root_after) = verify_witness(&w.h.env, &close.witness, 1 << DEPTH).unwrap();
    assert_eq!(root, close.header.state_root);
    assert_eq!(root_after, close.header.state_root_after);

    // Four changed accounts supply paired leaves; the untouched positions
    // collapse into shared frontier digests.
    assert_eq!(close.witness.changed.len(), 4);
    assert!(close.witness.frontier.len() < (1 << DEPTH) - 4 + 1);
}
