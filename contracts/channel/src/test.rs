#![cfg(test)]

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    xdr::ToXdr,
    Address, BytesN, Env,
};

use crate::{Commitment, Contract, ContractClient};

fn create_token<'a>(env: &Env) -> (Address, TokenClient<'a>, StellarAssetClient<'a>) {
    let admin = Address::generate(env);
    let contract_id = env.register_stellar_asset_contract_v2(admin.clone());
    let address = contract_id.address();
    (address.clone(), TokenClient::new(env, &address), StellarAssetClient::new(env, &address))
}

fn sign_commitment(env: &Env, signing_key: &SigningKey, channel: &Address, amount: i128) -> BytesN<64> {
    let commitment = Commitment {
        prefix: symbol_short!("chancmmt"),
        channel: channel.clone(),
        amount,
    };
    let payload = commitment.to_xdr(env);
    let buf = payload.to_buffer::<256>();
    let sig = signing_key.sign(buf.as_slice());
    BytesN::from_array(env, &sig.to_bytes())
}

#[test]
fn test_close_full_refund() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[1u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);
    let close_ledger_count: u32 = 100;

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, close_ledger_count));
    let client = ContractClient::new(&env, &channel_id);

    assert_eq!(token.balance(&channel_id), 500);
    assert_eq!(token.balance(&funder), 500);

    client.close(&0);

    env.ledger().with_mut(|li| {
        li.sequence_number += close_ledger_count + 1;
    });

    // close_start closes with amount 0, so full refund.
    client.refund();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

#[test]
fn test_close_refund_too_early() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[3u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    client.close(&0);

    let result = client.try_refund();
    assert!(result.is_err());
}

#[test]
fn test_close_dispute() {
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

    // Funder starts close (would result in full refund).
    client.close(&0);

    // Recipient disputes with close_with_commitment for 300.
    let sig = sign_commitment(&env, &auth_key, &channel_id, 300);
    client.close_with_commitment(&300, &sig);

    client.withdraw();
    client.refund();
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

#[test]
fn test_invalid_signature() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[4u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let wrong_key = SigningKey::from_bytes(&[5u8; 32]);

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = sign_commitment(&env, &wrong_key, &channel_id, 200);
    let result = client.try_close_with_commitment(&200, &sig);
    assert!(result.is_err());
}

#[test]
fn test_close_with_commitment() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[5u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = sign_commitment(&env, &auth_key, &channel_id, 300);
    client.close_with_commitment(&300, &sig);

    assert_eq!(token.balance(&channel_id), 500);

    client.withdraw();
    client.refund();
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

#[test]
fn test_withdraw_before_close_fails() {
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

    let result = client.try_withdraw();
    assert!(result.is_err());
}

#[test]
fn test_refund_before_close() {
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

    // Refund before close should fail.
    let result = client.try_refund();
    assert!(result.is_err());
}

#[test]
fn test_refund_before_withdraw() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[8u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    let sig = sign_commitment(&env, &auth_key, &channel_id, 300);
    client.close_with_commitment(&300, &sig);

    // Refund after close but before withdraw returns only the non-closed portion.
    client.refund();
    assert_eq!(token.balance(&funder), 700); // 500 kept + 200 refunded
    assert_eq!(token.balance(&channel_id), 300); // closed amount reserved

    // Withdraw still works for the closed amount.
    client.withdraw();
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&channel_id), 0);
}
