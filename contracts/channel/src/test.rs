#![cfg(test)]

use ed25519_dalek::SigningKey;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, BytesN, Env,
};

use crate::{Commitment, Contract, ContractClient};

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

/// Close transfers the committed amount from the channel to the recipient
/// and refunds the remainder to the funder.
#[test]
fn test_close() {
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

    let auth_key = SigningKey::from_bytes(&[3u8; 32]);
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

    let auth_key = SigningKey::from_bytes(&[4u8; 32]);
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

    let auth_key = SigningKey::from_bytes(&[5u8; 32]);
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

    let auth_key = SigningKey::from_bytes(&[6u8; 32]);
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

/// Close fails if the commitment signature does not match the commitment
/// key stored in the channel.
#[test]
fn test_invalid_signature() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[7u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let wrong_key = SigningKey::from_bytes(&[8u8; 32]);

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

/// Close fails after the close_start effective ledger has been reached.
#[test]
fn test_close_fails_after_close_start_effective() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[9u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    // Funder starts close.
    client.close_start();

    // Wait for close to become effective.
    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    // Recipient cannot close after the effective ledger has been reached.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    let result = client.try_close(&300, &sig);
    assert!(result.is_err());
}

/// The funder can top up the channel after creation.
#[test]
fn test_top_up_after_creation() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[10u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 300i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    assert_eq!(token.balance(&channel_id), 300);
    assert_eq!(token.balance(&funder), 700);

    // Top up with 200 more.
    client.top_up(&200);
    assert_eq!(token.balance(&channel_id), 500);
    assert_eq!(token.balance(&funder), 500);
}

/// Closing with a commitment for amount 0 refunds the full balance to the
/// funder.
#[test]
fn test_close_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[11u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Close with amount 0 — no transfer to recipient, but refund to funder.
    let sig = Commitment::new(channel_id.clone(), 0).sign(&auth_key);
    client.close(&0, &sig);
    assert_eq!(token.balance(&to), 0);
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Calling close_start again resets the waiting period, preventing refund until the
/// new waiting period elapses.
#[test]
fn test_close_start_resets_waiting_period() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[12u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    // First close_start.
    client.close_start();

    // Advance partway through the waiting period.
    env.ledger().with_mut(|li| {
        li.sequence_number += 50;
    });

    // Close start again — resets the waiting period.
    client.close_start();

    // Advance the original waiting period — should not be enough since it was reset.
    env.ledger().with_mut(|li| {
        li.sequence_number += 60;
    });

    // Refund should fail — still within the new waiting period.
    let result = client.try_refund();
    assert!(result.is_err());

    // Advance past the new waiting period.
    env.ledger().with_mut(|li| {
        li.sequence_number += 50;
    });

    // Refund should now succeed.
    client.refund();
}

/// Calling refund a second time succeeds but transfers nothing since the
/// balance is already zero.
#[test]
fn test_refund_twice() {
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

    // First refund drains the balance.
    client.refund();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);

    // Second refund succeeds but transfers nothing.
    client.refund();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Top up with amount 0 is a no-op and does not require auth.
#[test]
fn test_top_up_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[14u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Top up with 0 — no transfer should occur, no auth required.
    client.top_up(&0);
    assert_eq!(client.balance(), 500);
    // Verify no auth was required by checking auths is empty for top_up(0).
    let auths = env.auths();
    assert!(auths.is_empty());
}

/// Refund succeeds when called at exactly the effective_at_ledger (boundary
/// condition for the waiting period check).
#[test]
fn test_refund_at_exact_effective_ledger() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[17u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    // Advance exactly to the effective_at_ledger (not past it).
    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period;
    });

    // Refund should succeed at exactly the effective ledger.
    client.refund();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

/// close_start fails after the close effective ledger has been reached.
#[test]
fn test_close_start_fails_after_effective() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[19u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let refund_waiting_period: u32 = 100;

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, refund_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close_start();

    // Advance past the waiting period.
    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    // close_start should fail — already closed.
    let result = client.try_close_start();
    assert!(result.is_err());
}

/// close_start fails after close has been called (since close sets effective
/// ledger to current ledger).
#[test]
fn test_close_start_fails_after_close() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[20u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig);

    // close_start should fail — already closed by recipient.
    let result = client.try_close_start();
    assert!(result.is_err());
}

/// After close, refund succeeds immediately since close sets the effective
/// ledger to the current ledger.
#[test]
fn test_refund_after_close() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[18u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Close sets the effective ledger to now, so refund should work immediately.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.close(&300, &sig);

    // Refund succeeds immediately (balance is already 0 from auto-refund, but no error).
    client.refund();
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 700);
}
