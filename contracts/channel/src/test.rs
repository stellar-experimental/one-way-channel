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

/// Withdraw transfers the committed amount from the channel to the recipient.
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

/// Withdrawing with increasing commitment amounts only transfers the
/// incremental difference each time.
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

/// The funder can start and finish a close after the waiting period elapses.
#[test]
fn test_close_start_and_finish() {
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

    client.close_finish();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Close finish fails if called before the refund waiting period has elapsed.
#[test]
fn test_close_finish_too_early() {
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

    let result = client.try_close_finish();
    assert!(result.is_err());
}

/// Close finish fails if close_start has never been called.
#[test]
fn test_close_finish_before_start_fails() {
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

    let result = client.try_close_finish();
    assert!(result.is_err());
}

/// The recipient can withdraw during the close waiting period, and the funder
/// only refunds the remainder after the period elapses.
#[test]
fn test_withdraw_during_close_start() {
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

    // Recipient withdraws during the waiting period.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.withdraw(&300, &sig);
    assert_eq!(token.balance(&to), 300);

    // After wait, funder refunds the remainder.
    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    client.close_finish();
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Withdraw fails if the commitment signature does not match the commitment
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
    let result = client.try_withdraw(&200, &sig);
    assert!(result.is_err());
}

/// The recipient can withdraw after the close waiting period has elapsed, as
/// long as close_finish has not been called yet.
#[test]
fn test_withdraw_after_close_start_effective_before_finish() {
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

    // Funder closes.
    client.close_start();

    // Wait for close to become effective.
    env.ledger().with_mut(|li| {
        li.sequence_number += refund_waiting_period + 1;
    });

    // Recipient can still withdraw after close is effective, before refund.
    let sig = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.withdraw(&300, &sig);
    assert_eq!(token.balance(&to), 300);

    // Funder refunds the remainder.
    client.close_finish();
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
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

/// Withdrawing with a commitment for amount 0 is a no-op.
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

/// Calling close_start again resets the waiting period, preventing
/// close_finish until the new waiting period elapses.
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

    // First close.
    client.close_start();

    // Advance partway through the waiting period.
    env.ledger().with_mut(|li| {
        li.sequence_number += 50;
    });

    // Close again — resets the waiting period.
    client.close_start();

    // Advance the original waiting period — should not be enough since it was reset.
    env.ledger().with_mut(|li| {
        li.sequence_number += 60;
    });

    // Refund should fail — still within the new waiting period.
    let result = client.try_close_finish();
    assert!(result.is_err());

    // Advance past the new waiting period.
    env.ledger().with_mut(|li| {
        li.sequence_number += 50;
    });

    // Refund should now succeed.
    client.close_finish();
}

/// Calling close_finish a second time succeeds but transfers nothing since
/// the balance is already zero.
#[test]
fn test_close_finish_twice() {
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
    client.close_finish();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);

    // Second refund succeeds but transfers nothing.
    client.close_finish();
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

/// The deposited, balance, and withdrawn getters return correct values as the
/// channel state changes through withdrawals and top ups.
#[test]
fn test_deposited_and_balance_and_withdrawn() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[15u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, _token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    // Initial state.
    assert_eq!(client.deposited(), 500);
    assert_eq!(client.balance(), 500);
    assert_eq!(client.withdrawn(), 0);

    // After withdraw.
    let sig = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.withdraw(&200, &sig);
    assert_eq!(client.deposited(), 500);
    assert_eq!(client.balance(), 300);
    assert_eq!(client.withdrawn(), 200);

    // After top up.
    client.top_up(&100);
    assert_eq!(client.deposited(), 600);
    assert_eq!(client.balance(), 400);
    assert_eq!(client.withdrawn(), 200);
}

/// Using an older commitment with a lower amount after a higher amount has
/// been withdrawn is a no-op — no transfer and no state change.
#[test]
fn test_withdraw_older_commitment_no_op() {
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

    // Withdraw 300.
    let sig1 = Commitment::new(channel_id.clone(), 300).sign(&auth_key);
    client.withdraw(&300, &sig1);
    assert_eq!(token.balance(&to), 300);

    // Use an older commitment for 200 — no additional transfer, no state change.
    let sig2 = Commitment::new(channel_id.clone(), 200).sign(&auth_key);
    client.withdraw(&200, &sig2);
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&channel_id), 200);
    assert_eq!(client.withdrawn(), 300);
}

/// Both parties can cooperatively close in a single call, bypassing the
/// close_start/wait/close_finish flow.
#[test]
fn test_close() {
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

    client.close(&300);
    assert_eq!(token.balance(&to), 300);
    assert_eq!(token.balance(&funder), 700);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Close with zero withdraw amount sends everything to the funder.
#[test]
fn test_close_zero_withdraw() {
    let env = Env::default();
    env.mock_all_auths();

    let auth_key = SigningKey::from_bytes(&[19u8; 32]);
    let auth_pubkey = BytesN::from_array(&env, &auth_key.verifying_key().to_bytes());

    let to = Address::generate(&env);
    let funder = Address::generate(&env);

    let (token_addr, token, asset_admin) = create_token(&env);
    asset_admin.mint(&funder, &1000);

    let channel_id = env.register(Contract, (token_addr.clone(), funder.clone(), auth_pubkey.clone(), to.clone(), 500i128, 100u32));
    let client = ContractClient::new(&env, &channel_id);

    client.close(&0);
    assert_eq!(token.balance(&to), 0);
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}

/// Close finish succeeds when called at exactly the effective_at_ledger
/// (boundary condition for the waiting period check).
#[test]
fn test_close_finish_at_exact_effective_ledger() {
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
    client.close_finish();
    assert_eq!(token.balance(&funder), 1000);
    assert_eq!(token.balance(&channel_id), 0);
}
