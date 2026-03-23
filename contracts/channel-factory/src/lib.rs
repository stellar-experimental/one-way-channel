//! # Channel Factory
//!
//! A factory contract for opening channel contracts on Soroban (Stellar).
//!
//! The factory stores a channel contract wasm hash and opens new channel
//! instances using it. An admin can update the wasm hash to open newer
//! versions of the channel contract.
//!
//! ## Functions
//!
//! | Function | Description |
//! |---|---|
//! | `__constructor` | Initialize the factory with an admin and channel wasm hash. |
//! | `set_wasm` | Update the stored channel wasm hash. Admin only. |
//! | `open` | Deploy a new channel contract with the given parameters. |
//! | `admin` | Returns the admin address. |
//! | `wasm_hash` | Returns the stored channel wasm hash. |

#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env};

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
    /// Callable by the opener.
    ///
    /// # Auth
    /// None.
    pub fn __constructor(env: &Env, admin: Address, wasm_hash: BytesN<32>) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::WasmHash, &wasm_hash);
    }

    /// Returns the admin address.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn admin(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    /// Returns the stored channel contract wasm hash.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn wasm_hash(env: &Env) -> BytesN<32> {
        env.storage().instance().get(&DataKey::WasmHash).unwrap()
    }

    /// Update the stored channel contract wasm hash.
    ///
    /// Callable by the admin.
    ///
    /// # Auth
    /// - `admin`: required.
    pub fn set_wasm(env: &Env, wasm_hash: BytesN<32>) {
        // Verify the admin.
        let admin = Self::admin(env);
        admin.require_auth();

        env.storage().instance().set(&DataKey::WasmHash, &wasm_hash);
    }

    /// Deploy a new channel.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// - `from`: required if amount > 0.
    pub fn open(env: &Env, salt: BytesN<32>, token: Address, from: Address, commitment_key: BytesN<32>, to: Address, amount: i128, refund_waiting_period: u32) -> Address {
        if amount > 0 {
            // Authorize the funder at the factory level so that the channel
            // constructor's top_up does not require non-root authorization.
            from.require_auth();
        }

        // Deploy the channel contract using the stored wasm hash.
        let wasm_hash = Self::wasm_hash(env);
        let channel_address = env
            .deployer()
            .with_current_contract(salt)
            .deploy_v2(wasm_hash, (token, from, commitment_key, to, amount, refund_waiting_period));

        env.events().publish_event(&event::Open { channel: channel_address.clone() });

        channel_address
    }
}

pub mod event;
mod test;
