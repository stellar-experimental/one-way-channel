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

#[test]
fn test_withdraw() {
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
    client.withdraw(&300, &sig);

    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&channel_id), 200);
}

#[test]
fn test_withdraw_incremental() {
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

    // Withdraw 200 first.
    let sig1 = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.withdraw(&200, &sig1);
    assert_eq!(token.balance(&to), 200);

    // Withdraw 300 total — only 100 more transferred.
    let sig2 = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.withdraw(&300, &sig2);
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&channel_id), 200);
}

#[test]
fn test_close_and_refund() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[3u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let close_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, close_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close();

    env.ledger().with_mut(|li| {
        li.sequence_number += close_waiting_period + 1;
    });

    client.refund();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

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

    client.close();

    let result = client.try_refund();
    assert!(result.is_err());
}

#[test]
fn test_refund_before_close_fails() {
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

#[test]
fn test_withdraw_during_close() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[6u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let close_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, close_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    // Funder starts close.
    client.close();

    // Recipient withdraws during the waiting period.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.withdraw(&300, &sig);
    assert_eq!(token.balance(&to), 300);

    // After wait, funder refunds the remainder.
    env.ledger().with_mut(|li| {
        li.sequence_number += close_waiting_period + 1;
    });

    client.refund();
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

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
    let result = client.try_withdraw(&200, &sig);
    assert!(result.is_err());
}

#[test]
fn test_withdraw_after_close_effective_before_refund() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[9u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let close_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, close_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    // Funder closes.
    client.close();

    // Wait for close to become effective.
    env.ledger().with_mut(|li| {
        li.sequence_number += close_waiting_period + 1;
    });

    // Recipient can still withdraw after close is effective, before refund.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.withdraw(&300, &sig);
    assert_eq!(token.balance(&to), 300);

    // Funder refunds the remainder.
    client.refund();
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

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

#[test]
fn test_withdraw_zero_amount() {
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

    // Withdraw with amount 0 — no transfer.
    let sig = Commitment::new(channel_id.clone(), 0).sign(&auth_key);
    client.withdraw(&0, &sig);
    assert_eq!(token.balance(&to), 0);
    assert_eq!(token.balance(&channel_id), 500);
}

#[test]
fn test_close_resets_waiting_period() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[12u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let close_waiting_period: u32 = 100;

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, close_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    // First close.
    client.close();

    // Advance partway through the waiting period.
    env.ledger().with_mut(|li| {
        li.sequence_number += 50;
    });

    // Close again — resets the waiting period.
    client.close();

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

#[test]
fn test_refund_twice() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[13u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let close_waiting_period: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, close_waiting_period));
    let client = ContractClient::new(&env, &channel_id);

    client.close();

    env.ledger().with_mut(|li| {
        li.sequence_number += close_waiting_period + 1;
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
