#![cfg(test)]

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    xdr::ToXdr,
    Address, BytesN, Env,
};

use crate::{Contract, ContractClient, Voucher};

fn create_token<'a>(env: &Env) -> (Address, TokenClient<'a>, StellarAssetClient<'a>) {
    let admin = Address::generate(env);
    let contract_id = env.register_stellar_asset_contract_v2(admin.clone());
    let address = contract_id.address();
    (address.clone(), TokenClient::new(env, &address), StellarAssetClient::new(env, &address))
}

fn sign_voucher(env: &Env, signing_key: &SigningKey, channel: &Address, token: &Address, amount: i128) -> BytesN<64> {
    let voucher = Voucher {
        prefix: symbol_short!("chanvchr"),
        channel: channel.clone(),
        token: token.clone(),
        amount,
    };
    let payload = voucher.to_xdr(env);
    let buf = payload.to_buffer::<256>();
    let sig = signing_key.sign(buf.as_slice());
    BytesN::from_array(env, &sig.to_bytes())
}

#[test]
fn test_full_flow() {
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

    let sig = sign_voucher(&env, &auth_key, &channel_id, &token_addr, 400);
    client.close_start(&400, &sig);

    env.ledger().with_mut(|li| {
        li.sequence_number += close_ledger_count + 1;
    });

    client.close_finish();
    assert_eq!(token.balance(&channel_id), 500);

    client.withdraw();
    assert_eq!(token.balance(&to), 400);
    assert_eq!(token.balance(&channel_id), 100);

    client.refund();
    assert_eq!(token.balance(&funder), 600);
    assert_eq!(token.balance(&channel_id), 0);
}

#[test]
fn test_close_dispute_overwrites() {
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

    let sig1 = sign_voucher(&env, &auth_key, &channel_id, &token_addr, 100);
    client.close_start(&100, &sig1);

    let sig2 = sign_voucher(&env, &auth_key, &channel_id, &token_addr, 300);
    client.close_start(&300, &sig2);

    env.ledger().with_mut(|li| {
        li.sequence_number += 101;
    });

    client.close_finish();
    client.withdraw();
    client.refund();
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

#[test]
fn test_close_finish_too_early() {
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

    let sig = sign_voucher(&env, &auth_key, &channel_id, &token_addr, 200);
    client.close_start(&200, &sig);

    let result = client.try_close_finish();
    assert!(result.is_err());
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

    let sig = sign_voucher(&env, &wrong_key, &channel_id, &token_addr, 200);
    let result = client.try_close_start(&200, &sig);
    assert!(result.is_err());
}

#[test]
fn test_close_immediately() {
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

    let sig = sign_voucher(&env, &auth_key, &channel_id, &token_addr, 300);
    client.close_immediately(&300, &sig);

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
