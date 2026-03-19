//! # Channel
//!
//! A unidirectional payment channel contract for Soroban (Stellar).
//!
//! A payment channel allows a funder to make many small payments to a recipient
//! off-chain, with only two on-chain transactions: opening the channel and
//! closing it. This avoids per-payment transaction fees and latency.
//!
//! ## Participants
//!
//! - **Funder (`from`)**: Deposits tokens into the channel and signs
//!   commitments authorizing the recipient to withdraw up to a given cumulative
//!   amount.
//! - **Recipient (`to`)**: Receives commitments off-chain and can withdraw
//!   funds on-chain at any time using a signed commitment.
//!
//! ## Lifecycle
//!
//! ### 1. Open
//!
//! The channel is deployed with a SEP-41 token, funder address, recipient
//! address, an ed25519 `commitment_key` (public key), an initial deposit
//! amount, and a `close_waiting_period` (in ledgers).
//!
//! The funder's tokens are transferred into the channel contract on deployment.
//! The funder can also top up the channel later using [`Contract::top_up`], or
//! by transferring the token directly to the channel contract address.
//!
//! ### 2. Off-chain payments
//!
//! The funder makes payments by signing commitments off-chain and sending them
//! to the recipient. A commitment authorizes the recipient to withdraw up to a
//! **cumulative total** amount. Each new commitment replaces the previous one.
//!
//! For example:
//! - Commitment for 100: recipient can withdraw up to 100.
//! - Commitment for 140: recipient can withdraw up to 140 (40 more if 100 was
//!   already withdrawn).
//! - Commitment for 300: recipient can withdraw up to 300 (160 more if 140 was
//!   already withdrawn).
//!
//! A commitment is an XDR serialized [`Commitment`] struct containing a domain
//! separator (`chancmmt`), the channel contract address, and the amount. The
//! funder signs the serialized bytes with the ed25519 key corresponding to the
//! `commitment_key`. Use [`Contract::prepare_commitment`] as a convenience to
//! generate the bytes to sign.
//!
//! The serialized commitment is an XDR `ScVal::Map` with three entries:
//!
//! ```text
//! ScVal::Map({
//!     Symbol("amount"):  I128(amount),
//!     Symbol("channel"): Address(channel_contract_address),
//!     Symbol("domain"):  Symbol("chancmmt"),
//! })
//! ```
//!
//! The XDR bytes of a commitment for amount 100 with a zero channel address
//! look like:
//!
//! ```text
//! 00000000  00 00 00 0e 00 00 00 03  00 00 00 0f 00 00 00 06  |................|
//!           \-- ScVal  | \-- 3 entries | \-- key:  | \-- 6 chars
//!              Map     |               |    Symbol |
//! 00000010  61 6d 6f 75 6e 74 00 00  00 00 00 0a 00 00 00 00  |amount....\-----|
//!           |--- "amount" ---|        | \-- val: I128          |
//! 00000020  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 64  |...............d|
//!           |-------------- hi: 0 ----------| |--- lo: 100 ---|
//! 00000030  00 00 00 0f 00 00 00 07  63 68 61 6e 6e 65 6c 00  |........channel.|
//!           | \-- key:  | \-- 7 chars |-- "channel" --|
//!           |    Symbol |
//! 00000040  00 00 00 12 00 00 00 00  00 00 00 00 00 00 00 00  |................|
//!           | \-- val:   |------------ 32-byte ---------------|
//!           |    Address
//! 00000050  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00  |................|
//! 00000060  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00  |................|
//!           |------------- contract address ---------------|
//! 00000070  00 00 00 0f 00 00 00 06  64 6f 6d 61 69 6e 00 00  |........domain..|
//!           | \-- key:  | \-- 6 chars |--- "domain" ---|
//!           |    Symbol |
//! 00000080  00 00 00 0f 00 00 00 08  63 68 61 6e 63 6d 6d 74  |........chancmmt|
//!           | \-- val:  | \-- 8 chars |--- "chancmmt" -|
//!           |    Symbol |
//! ```
//!
//! ### 3. Withdraw
//!
//! The recipient calls [`Contract::withdraw`] at any time with a commitment
//! amount and its signature. The contract verifies the signature, then
//! transfers the difference between the commitment amount and what has already
//! been withdrawn. If the commitment amount is less than or equal to what has
//! already been withdrawn, no transfer occurs.
//!
//! The recipient does not need to withdraw after every commitment. They can
//! accumulate multiple commitments and withdraw using only the latest (highest
//! amount) commitment.
//!
//! ### 4. Close
//!
//! The funder calls [`Contract::close`] to begin closing the channel. The close
//! does not take effect immediately — there is a waiting period of
//! `close_waiting_period` ledgers.
//!
//! The recipient can still call [`Contract::withdraw`] at any time, including
//! after the waiting period has elapsed, up until the funder calls
//! [`Contract::refund`]. However, once the waiting period has elapsed the
//! funder can call refund at any time, so the recipient should withdraw
//! promptly.
//!
//! **Important:** The recipient should monitor for [`event::Close`] events and
//! withdraw before the close becomes effective.
//!
//! ### 5. Refund
//!
//! After the close waiting period has elapsed, the funder calls
//! [`Contract::refund`] to reclaim whatever balance remains in the channel.
//! This transfers the **entire** remaining token balance to the funder,
//! including any amount the recipient was entitled to but did not withdraw.
//! The contract does not reserve funds for the recipient. If the recipient
//! has not withdrawn before the funder calls refund, those funds are lost to
//! the recipient and assumed to be of no interest to the recipient.
//!
//! ## Security
//!
//! - Commitments are signed with an ed25519 key, not a Stellar account. The
//!   `commitment_key` is set at deployment and cannot be changed.
//! - The commitment includes a domain separator and the channel contract
//!   address, preventing signatures from being reused across channels or
//!   confused with other signed payloads.
//! - The close waiting period protects the recipient: it gives them time to
//!   withdraw using their latest commitment before the funder can reclaim
//!   funds.

#![no_std]
#[allow(unused_imports)]
use soroban_sdk::{assert_with_error, contract, contracterror, contractimpl, contracttype, symbol_short, token, xdr::ToXdr, Address, Bytes, BytesN, Env, Symbol};

mod event;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NegativeAmount = 1,
    NotClosed = 2,
    CloseWaitingPeriodNotElapsed = 3,
}

#[contracttype]
pub enum DataKey {
    Token,
    From,
    CommitmentKey,
    To,
    CloseWaitingPeriod,
    Withdrawn,
    Closed,
}

#[contracttype]
pub struct Commitment {
    domain: Symbol,
    channel: Address,
    amount: i128,
}

impl Commitment {
    pub fn new(channel: Address, amount: i128) -> Self {
        Commitment {
            domain: symbol_short!("chancmmt"),
            channel,
            amount,
        }
    }

    fn into_bytes(&self) -> Bytes {
        let env = self.channel.env();
        self.to_xdr(env)
    }

    fn verify(self, sig: &BytesN<64>) {
        let env = self.channel.env().clone();
        let commitment_key: BytesN<32> = env.storage().instance().get(&DataKey::CommitmentKey).unwrap();
        let payload = self.into_bytes();
        env.crypto().ed25519_verify(&commitment_key, &payload, sig);
    }
}

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    /// Open a channel by depositing tokens from the funder to the contract.
    ///
    /// - `token`: The SEP-41 token used for payments.
    /// - `from`: The funder who deposits tokens into the channel.
    /// - `commitment_key`: The ed25519 public key used to verify commitment
    ///   signatures. See [`Self::prepare_commitment`] for details on
    ///   commitments.
    /// - `to`: The recipient who can [`Self::withdraw`] funds using signed
    ///   commitments.
    /// - `amount`: The initial deposit amount.
    /// - `close_waiting_period`: The number of ledgers the recipient has to
    ///   withdraw after [`Self::close`] is called, before [`Self::refund`]
    ///   becomes available.
    ///
    /// Callable by the deployer.
    ///
    /// # Auth
    /// - `from`: required if amount > 0.
    pub fn __constructor(env: &Env, token: Address, from: Address, commitment_key: BytesN<32>, to: Address, amount: i128, close_waiting_period: u32) {
        assert_with_error!(&env, amount >= 0, Error::NegativeAmount);

        // Store channel configuration.
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::From, &from);
        env.storage().instance().set(&DataKey::CommitmentKey, &commitment_key);
        env.storage().instance().set(&DataKey::To, &to);
        env.storage().instance().set(&DataKey::CloseWaitingPeriod, &close_waiting_period);

        // Deposit initial funds.
        Self::top_up(env, amount);

        env.events().publish_event(&event::Open {
            from,
            commitment_key,
            to,
            token,
            amount,
            close_waiting_period,
        });
    }

    /// Top up the channel by transferring the amount of the channels token from the funder (from
    /// address).
    ///
    /// Note: The funder can also top up the channel by transferring tokens
    /// directly to the channel contract address outside of this function.
    ///
    /// Callable by funder (from).
    ///
    /// # Auth
    /// - `from`: required.
    pub fn top_up(env: &Env, amount: i128) {
        assert_with_error!(&env, amount >= 0, Error::NegativeAmount);
        if amount > 0 {
            // Transfer tokens from the funder to the channel.
            let from = Self::from(env);
            from.require_auth();
            Self::token_client(env).transfer(&from, &env.current_contract_address(), &amount);
        }
    }

    /// Returns the token address.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn token(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Token).unwrap()
    }

    /// Returns the funder address.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn from(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::From).unwrap()
    }

    /// Returns the recipient address.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn to(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::To).unwrap()
    }

    /// Returns the close waiting period in ledgers.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn close_waiting_period(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::CloseWaitingPeriod).unwrap()
    }

    /// Returns the total amount deposited into the channel.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn deposited(env: &Env) -> i128 {
        Self::balance(env) + Self::withdrawn(env)
    }

    /// Returns the total amount already withdrawn by the recipient.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn withdrawn(env: &Env) -> i128 {
        env.storage().instance().get(&DataKey::Withdrawn).unwrap_or(0)
    }

    /// Returns the balance of the channel.
    ///
    /// This is the deposited amount minus any amount already withdrawn.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn balance(env: &Env) -> i128 {
        Self::token_client(env).balance(&env.current_contract_address())
    }

    /// Returns the XDR serialized bytes of a commitment for the given amount.
    ///
    /// The amount is the total cumulative amount the recipient is entitled to
    /// withdraw, not an incremental amount. Each new commitment replaces the
    /// previous one. For example: if the funder gives the recipient a signed
    /// commitment for 100, the recipient can withdraw up to 100. If the
    /// recipient withdraws using the committment they will receive 100
    /// immediately. If the funder wishes to send a payment of 40 to the
    /// receipient, the funder gives the recipient a signed committment for 140.
    /// If the recipient withdraws using the committment of 140, they receive 40
    /// immediately.
    ///
    /// The returned bytes must be signed by the ed25519 key corresponding to
    /// the `commitment_key` stored in the channel. The resulting signature,
    /// along with the amount, can be passed to [`Self::withdraw`] by the
    /// recipient to withdraw funds at any time.
    ///
    /// Commitments are typically prepared off-chain. This function is provided
    /// as a convenience.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_commitment(env: &Env, amount: i128) -> Bytes {
        assert_with_error!(&env, amount >= 0, Error::NegativeAmount);
        Commitment::new(env.current_contract_address(), amount).into_bytes()
    }

    /// Withdraw funds to the recipient using a signed commitment. The amount is
    /// the total amount the recipient is entitled to. Only the difference
    /// between the amount and what has already been withdrawn is transferred.
    /// Can be called at any time.
    ///
    /// The withdrawal amount is not configurable. Each call withdraws exactly
    /// the amount needed to bring the total withdrawn up to the amount
    /// authorized by the signed commitment. If an older commitment with a
    /// lower amount is used after a higher amount has already been withdrawn,
    /// no funds are transferred.
    ///
    /// **Important:** The recipient should call this whenever they see a
    /// [`event::Close`], before the close becomes effective. After the close is
    /// effective the funder can refund the remaining balance.
    ///
    /// Callable by the recipient (to).
    ///
    /// # Auth
    /// - `to`: required.
    /// - Commitment signature serves as commitment_key authorization.
    pub fn withdraw(env: &Env, amount: i128, sig: BytesN<64>) {
        assert_with_error!(&env, amount >= 0, Error::NegativeAmount);

        // Verify the recipient and commitment signature.
        let to = Self::to(env);
        to.require_auth();
        Commitment::new(env.current_contract_address(), amount).verify(&sig);

        // Transfer only the difference from what has already been withdrawn.
        let payout = amount - Self::withdrawn(env);
        if payout > 0 {
            env.storage().instance().set(&DataKey::Withdrawn, &amount);
            Self::token_client(env).transfer(&env.current_contract_address(), &to, &payout);
            env.events().publish_event(&event::Withdraw { to, amount: payout });
        }
    }

    /// Close the channel, effective after a waiting period. The recipient can
    /// still withdraw during the waiting period. After the close is effective,
    /// the funder can call refund to reclaim the remaining balance.
    ///
    /// **Important:** The recipient should withdraw funds using [`Self::withdraw`]
    /// whenever they see a [`event::Close`], before the close becomes effective.
    /// After the close is effective the funder can refund the remaining balance.
    /// The recipient can still withdraw even after the close is effective, up
    /// until [`Self::refund`] is called.
    ///
    /// Callable by the funder (from).
    ///
    /// # Auth
    /// - `from`: required.
    pub fn close(env: &Env) {
        // Verify the funder.
        let from = Self::from(env);
        from.require_auth();

        // Set the close effective ledger.
        let close_waiting_period = Self::close_waiting_period(env);
        let effective_at_ledger = env.ledger().sequence() + close_waiting_period;
        env.storage().instance().set(&DataKey::Closed, &effective_at_ledger);

        env.events().publish_event(&event::Close { effective_at_ledger });
    }

    /// Refund the remaining balance to the funder after the close is effective.
    ///
    /// Callable by the funder (from), after the close is effective_at_ledger
    /// has been reached.
    ///
    /// # Auth
    /// - `from`: required.
    pub fn refund(env: &Env) -> Result<(), Error> {
        // Verify the close is effective.
        let effective_at_ledger: u32 = env.storage().instance().get(&DataKey::Closed).ok_or(Error::NotClosed)?;
        if env.ledger().sequence() < effective_at_ledger {
            return Err(Error::CloseWaitingPeriodNotElapsed);
        }

        // Verify the funder.
        let from = Self::from(env);
        from.require_auth();

        // Transfer the remaining balance to the funder.
        let tc = Self::token_client(env);
        let balance = tc.balance(&env.current_contract_address());
        if balance > 0 {
            tc.transfer(&env.current_contract_address(), &from, &balance);
            env.events().publish_event(&event::Refund { from, amount: balance });
        }
        Ok(())
    }
}

impl Contract {
    fn token_client(env: &Env) -> token::Client<'_> {
        token::Client::new(env, &Self::token(env))
    }
}

mod test;
