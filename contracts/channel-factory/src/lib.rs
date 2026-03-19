#![no_std]
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, BytesN, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotAdmin = 1,
}

#[contracttype]
pub enum DataKey {
    Admin,
    WasmHash,
}

#[contract]
pub struct FactoryContract;

#[contractimpl]
impl FactoryContract {
    /// Initialize the factory with an admin and a channel contract wasm hash.
    ///
    /// Callable by the deployer.
    ///
    /// # Auth
    /// None.
    pub fn __constructor(env: &Env, admin: Address, wasm_hash: BytesN<32>) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::WasmHash, &wasm_hash);
    }

    /// Update the stored channel contract wasm hash.
    ///
    /// Callable by the admin.
    ///
    /// # Auth
    /// - `admin`: required.
    pub fn set_wasm_hash(env: &Env, wasm_hash: BytesN<32>) {
        // Verify the admin.
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        env.storage().instance().set(&DataKey::WasmHash, &wasm_hash);
    }

    /// Deploy a new channel contract.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// - `from`: required if amount > 0.
    pub fn deploy(env: &Env, salt: BytesN<32>, token: Address, from: Address, commitment_key: BytesN<32>, to: Address, amount: i128, refund_waiting_period: u32) -> Address {
        // Authorize the funder at the factory level so that the channel
        // constructor's top_up does not require non-root authorization.
        from.require_auth();

        // Deploy the channel contract using the stored wasm hash.
        let wasm_hash: BytesN<32> = env.storage().instance().get(&DataKey::WasmHash).unwrap();
        let channel_address = env
            .deployer()
            .with_current_contract(salt)
            .deploy_v2(wasm_hash, (token, from, commitment_key, to, amount, refund_waiting_period));

        channel_address
    }
}

mod test;
