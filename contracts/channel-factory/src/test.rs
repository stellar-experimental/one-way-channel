#![cfg(test)]

use ed25519_dalek::SigningKey;
use soroban_sdk::{
    testutils::Address as _,
    token::{StellarAssetClient, TokenClient},
    Address, BytesN, Env,
};

use crate::{FactoryContract, FactoryContractClient};

mod channel_contract {
    soroban_sdk::contractimport!(file = "../../target/wasm32v1-none/release/channel.wasm");
}

fn create_token<'a>(env: &Env) -> (Address, TokenClient<'a>, StellarAssetClient<'a>) {
    let admin = Address::generate(env);
    let contract_id = env.register_stellar_asset_contract_v2(admin.clone());
    let address = contract_id.address();
    (address.clone(), TokenClient::new(env, &address), StellarAssetClient::new(env, &address))
}

#[test]
fn test_deploy() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let wasm_hash = env.deployer().upload_contract_wasm(channel_contract::WASM);

    // Deploy the factory.
    let factory_id = env.register(FactoryContract, (&admin, &wasm_hash));
    let factory_client = FactoryContractClient::new(&env, &factory_id);

    // Set up a channel.
    let auth_key = SigningKey::from_bytes(&[1u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());
    let funder = Address::generate(&env);
    let to = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    // Deploy a channel via the factory.
    let salt = BytesN::from_array(&env, &[0u8; 32]);
    let channel_id = factory_client.deploy(&salt, &token_addr, &funder, &auth_pubkey, &to, &500i128, &100u32);

    // Verify the channel was funded.
    assert_eq!(token.balance(&channel_id), 500);
    assert_eq!(token.balance(&funder), 500);
}
