#![cfg(test)]

use ed25519_dalek::SigningKey;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    xdr, Address, BytesN, Env,
};

use crate::{Commitment, Contract, ContractClient};

fn has_event_type(env: &Env, contract: &Address, event_name: &str) -> bool {
    let events = env.events().all().filter_by_contract(contract);
    let target = xdr::ScVal::Symbol(xdr::ScSymbol(event_name.try_into().unwrap()));
    events.events().iter().any(|e| match &e.body {
        xdr::ContractEventBody::V0(body) => body.topics.first() == Some(&target),
    })
}

impl Commitment {
    fn sign(self, signing_key: &SigningKey) -> BytesN<64> {
        use ed25519_dalek::Signer;
        use soroban_sdk::xdr::ToXdr;
        let env = self.channel.env().clone();
        let payload = self.to_xdr(&env);
        let buf = payload.to_buffer::<256>();
        let sig = signing_key.sign(buf.as_slice());
        BytesN::from_array(&env, &sig.to_bytes())
    }
}

fn create_token<'a>(env: &Env) -> (Address, TokenClient<'a>, StellarAssetClient<'a>) {
    let admin = Address::generate(env);
    let contract_id = env.register_stellar_asset_contract_v2(admin.clone());
    let address = contract_id.address();
    (address.clone(), TokenClient::new(env, &address), StellarAssetClient::new(env, &address))
}

/// Settle transfers the committed amount from the channel to the recipient
/// without closing the channel.
#[test]
fn test_settle() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[1u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.settle(&300, &sig);

    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&channel_id), 200);
    assert_eq!(client.withdrawn(), 300);
}

/// Settling with increasing commitment amounts only transfers the
/// incremental difference each time.
#[test]
fn test_settle_incremental() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[2u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Settle 200 first.
    let sig1 = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.settle(&200, &sig1);
    assert_eq!(token.balance(&to), 200);
    assert_eq!(client.withdrawn(), 200);

    // Settle 300 total — only 100 more transferred.
    let sig2 = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.settle(&300, &sig2);
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&channel_id), 200);
    assert_eq!(client.withdrawn(), 300);
}

/// Using an older commitment with a lower amount after a higher amount has
/// been settled is a no-op.
#[test]
fn test_settle_older_commitment_no_op() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[16u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Settle 300.
    let sig1 = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.settle(&300, &sig1);
    assert_eq!(token.balance(&to), 300);

    // Use an older commitment for 200 — no additional transfer.
    let sig2 = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.settle(&200, &sig2);
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&channel_id), 200);
    assert_eq!(client.withdrawn(), 300);
}

/// Close after settle only transfers the difference.
#[test]
fn test_close_after_settle() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[3u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Settle 200 first.
    let sig1 = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.settle(&200, &sig1);
    assert_eq!(token.balance(&to), 200);

    // Close with 500 — only 300 more transferred, remainder refunded to funder.
    let sig2 = Commitment::new(channel_id.clone(), 500).sign(&auth_key);
    client.close(&500, &sig2);
    assert_eq!(token.balance(&to), 500);
    assert_eq!(token.balance(&channel_id), 0);
    assert_eq!(token.balance(&funder), 500);
    assert_eq!(client.withdrawn(), 500);
}

/// Close transfers the difference between committed amount and already
/// withdrawn, and refunds the remainder to the funder.
#[test]
fn test_close() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[4u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig);

    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&channel_id), 0);
    assert_eq!(token.balance(&funder), 700);
}

/// The funder can start closing the channel and refund the full balance after the
/// waiting period elapses.
#[test]
fn test_close_start_and_refund() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[5u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    client.refund();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Refund fails if called before the refund waiting period has elapsed.
#[test]
fn test_refund_too_early() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[6u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    let result = client.try_refund();
    assert!(result.is_err());
}

/// Refund fails if close_start has never been called.
#[test]
fn test_refund_before_close_start_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[7u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let result = client.try_refund();
    assert!(result.is_err());
}

/// The recipient can close during the refund waiting period, and the close
/// automatically refunds the remainder to the funder.
#[test]
fn test_close_during_close_start() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[8u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    // Funder starts close.
    client.close_start();

    // Recipient closes during the waiting period.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig);
    assert_eq!(token.balance(&to), 300);

    // Close automatically refunded the remainder to the funder.
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Settle works during the close_start waiting period.
#[test]
fn test_settle_during_close_start() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[9u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    // Settle during the waiting period — does not close the channel.
    let sig = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.settle(&200, &sig);
    assert_eq!(token.balance(&to), 200);
    assert_eq!(token.balance(&channel_id), 300);

    // Funder can still refund after the waiting period (gets remainder).
    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    client.refund();
    assert_eq!(token.balance(&funder), 800);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Settle works even after the channel is closed (after close_start effective
/// ledger reached), because settle does not check closed state.
#[test]
fn test_settle_after_close_start_effective() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[10u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    // Settle still works after the effective ledger.
    let sig = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.settle(&200, &sig);
    assert_eq!(token.balance(&to), 200);
    assert_eq!(token.balance(&channel_id), 300);
}

/// Close fails if the commitment signature does not match.
#[test]
fn test_invalid_signature() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[11u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let wrong_key = SigningKey::from_bytes(&[12u8; 32]);

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 200).sign(&wrong_key);
    let result = client.try_close(&200, &sig);
    assert!(result.is_err());
}

/// Close works after the close_start effective ledger has been reached,
/// as long as there is still balance.
#[test]
fn test_close_after_close_start_effective() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[13u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    // Close still works after the effective ledger.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig);
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

/// The funder can top up the channel after creation.
#[test]
fn test_top_up_after_creation() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[14u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 300i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    assert_eq!(token.balance(&channel_id), 300);
    assert_eq!(token.balance(&funder), 700);

    client.top_up(&200);
    assert_eq!(token.balance(&channel_id), 500);
    assert_eq!(token.balance(&funder), 500);
}

/// Closing with a commitment for amount 0 refunds the full balance to the funder.
#[test]
fn test_close_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[15u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 0).sign(&auth_key);
    client.close(&0, &sig);
    assert!(!has_event_type(&env, &channel_id, "withdraw"));
    assert_eq!(token.balance(&to), 0);
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Closing for the full balance transfers everything to the recipient and
/// does not emit a spurious Refund event.
#[test]
fn test_close_full_balance_no_refund_event() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[23u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 500).sign(&auth_key);
    client.close(&500, &sig);
    assert!(!has_event_type(&env, &channel_id, "refund"));
    assert_eq!(token.balance(&to), 500);
    assert_eq!(token.balance(&funder), 500);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Calling close_start again resets the waiting period.
#[test]
fn test_close_start_resets_waiting_period() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[17u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += 50;
    });

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += 60;
    });

    let result = client.try_refund();
    assert!(result.is_err());

    env.ledger().with_mut(|li| {
        li.sequence_number += 50;
    });

    client.refund();
}

/// Calling refund twice succeeds but the second transfers nothing.
#[test]
fn test_refund_twice() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[18u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    client.refund();
    assert_eq!(token.balance(&funder), 1000);

    client.refund();
    assert!(!has_event_type(&env, &channel_id, "refund"));
    assert_eq!(token.balance(&funder), 1000);
}

/// Top up with amount 0 is a no-op and does not require auth.
#[test]
fn test_top_up_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[19u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    client.top_up(&0);
    let auths = env.auths();
    assert!(auths.is_empty());
    assert_eq!(client.balance(), 500);
}

/// Refund succeeds at exactly the effective_at_ledger.
#[test]
fn test_refund_at_exact_effective_ledger() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[20u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period;
    });

    client.refund();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

/// close_start fails after the close effective ledger has been reached.
#[test]
fn test_close_start_fails_after_effective() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[21u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    let result = client.try_close_start();
    assert!(result.is_err());
}

/// close_start fails after close has been called.
#[test]
fn test_close_start_fails_after_close() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[22u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig);

    let result = client.try_close_start();
    assert!(result.is_err());
}

/// After close, refund succeeds immediately.
#[test]
fn test_refund_after_close() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[24u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig);

    client.refund();
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 700);
}

/// Calling close a second time succeeds but does not transfer more than
/// the cumulative committed amount.
#[test]
fn test_close_twice() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[25u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &2000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // First close: transfers 300, auto-refunds 200.
    let sig1 = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig1);
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 1700);

    // Top up again.
    client.top_up(&500);

    // Second close with higher commitment: transfers only the difference (200).
    let sig2 = Commitment::new(channel_id.clone(), 500).sign(&auth_key);
    client.close(&500, &sig2);
    assert_eq!(token.balance(&to), 500);
    assert_eq!(token.balance(&funder), 1500);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Reopen after close transitions the channel back to Open and allows new
/// commitments.
#[test]
fn test_reopen_after_close() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[27u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &2000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Close the channel.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig);
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 1700);

    // Reopen with a new deposit.
    client.reopen(&400);
    assert!(has_event_type(&env, &channel_id, "open"));
    assert_eq!(token.balance(&channel_id), 400);
    assert_eq!(token.balance(&funder), 1300);

    // Can settle again with a new commitment (amount must be > withdrawn).
    let sig2 = Commitment::new(channel_id.clone(), 600).sign(&auth_key);
    client.settle(&600, &sig2);
    assert_eq!(token.balance(&to), 600);
    assert_eq!(client.withdrawn(), 600);
}

/// Reopen after close_start + wait + refund works.
#[test]
fn test_reopen_after_refund() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[28u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &2000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    client.refund();
    assert_eq!(token.balance(&funder), 2000);
    assert_eq!(token.balance(&channel_id), 0);

    // Reopen with deposit.
    client.reopen(&300);
    assert_eq!(token.balance(&channel_id), 300);
    assert_eq!(token.balance(&funder), 1700);

    // Channel is open — close_start works again.
    client.close_start();
}

/// Reopen preserves withdrawn amount so old commitments cannot be replayed.
#[test]
fn test_reopen_preserves_withdrawn() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[29u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &2000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Settle 300.
    let sig1 = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.settle(&300, &sig1);
    assert_eq!(client.withdrawn(), 300);

    // Close with same 300 commitment (no additional payout).
    let sig2 = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig2);

    // Reopen.
    client.reopen(&500);
    assert_eq!(client.withdrawn(), 300);

    // Replaying the old 300 commitment transfers nothing.
    let sig3 = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.settle(&300, &sig3);
    assert_eq!(token.balance(&to), 300);
}

/// Reopen with zero amount deposits nothing.
#[test]
fn test_reopen_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[30u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 0).sign(&auth_key);
    client.close(&0, &sig);

    client.reopen(&0);
    assert_eq!(token.balance(&channel_id), 0);
    assert_eq!(token.balance(&funder), 1000);
}

/// Reopen fails if the channel is not closed.
#[test]
fn test_reopen_fails_when_open() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[31u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let result = client.try_reopen(&0);
    assert!(result.is_err());
}

/// Reopen fails during the close_start waiting period.
#[test]
fn test_reopen_fails_during_closing() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[32u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    let result = client.try_reopen(&0);
    assert!(result.is_err());
}

/// After reopen, close_start works again.
#[test]
fn test_close_start_works_after_reopen() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[33u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &2000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Close the channel.
    let sig = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.close(&200, &sig);

    // Reopen.
    client.reopen(&500);

    // close_start should work again.
    client.close_start();

    // After waiting, refund works.
    env.ledger().with_mut(|li| {
        li.sequence_number += 101;
    });

    client.refund();
    assert_eq!(token.balance(&funder), 1800);
}

/// Close panics if the commitment amount exceeds the channel balance.
#[test]
#[should_panic(expected = "balance is not sufficient")]
fn test_close_amount_exceeds_balance() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[26u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 600).sign(&auth_key);
    client.close(&600, &sig);
}
