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
//! ## State diagram
//!
//! ```mermaid
//! stateDiagram-v2
//!     [*] --> Open: __constructor
//!     Open --> Closing: close
//!     Closing --> Closed: [after wait]
//!     Closed --> [*]: refund
//! ```
//!
//! `top_up` and `withdraw` can be called in any state. After `refund` the
//! channel balance is zero so there is nothing left to withdraw.
//!
//! ## Functions
//!
//! ### Lifecycle
//!
//! | Function | Description |
//! |---|---|
//! | `__constructor` | Open a channel with an initial deposit. Callable by the funder, or anyone if amount is zero. |
//! | `top_up` | Deposit additional tokens into the channel. |
//! | `withdraw` | Withdraw funds using a signed commitment. |
//! | `close` | Begin closing the channel, effective after a waiting period. |
//! | `refund` | Refund the remaining balance to the funder after the close is effective. |
//!
//! ### Helpers
//!
//! | Function | Description |
//! |---|---|
//! | `prepare_commitment` | Generate the commitment bytes to sign. |
//!
//! ### Getters
//!
//! | Function | Description |
//! |---|---|
//! | `token` | Returns the token address. |
//! | `from` | Returns the funder address. |
//! | `to` | Returns the recipient address. |
//! | `refund_waiting_period` | Returns the refund waiting period in ledgers. |
//! | `deposited` | Returns the total amount deposited. |
//! | `withdrawn` | Returns the total amount already withdrawn. |
//! | `balance` | Returns the current balance (deposited minus withdrawn). |
//!
//! ## Lifecycle
//!
//! ### 1. Open
//!
//! The channel is deployed with a SEP-41 token, funder address, recipient
//! address, an ed25519 `commitment_key` (public key), an initial deposit
//! amount, and a `refund_waiting_period` (in ledgers).
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
//! separator (`chancmmt`), the network ID, the channel contract address, and
//! the amount. The
//! funder signs the serialized bytes with the ed25519 key corresponding to the
//! `commitment_key`. Use [`Contract::prepare_commitment`] as a convenience to
//! generate the bytes to sign.
//!
//! The serialized commitment is an XDR `ScVal::Map` with four entries
//! (sorted alphabetically by key):
//!
//! ```text
//! ScVal::Map({
//!     Symbol("amount"):  I128(amount),
//!     Symbol("channel"): Address(channel_contract_address),
//!     Symbol("domain"):  Symbol("chancmmt"),
//!     Symbol("network"): BytesN<32>(network_id),
//! })
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
//! `refund_waiting_period` ledgers.
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
//! After the refund waiting period has elapsed, the funder calls
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
//! - The commitment includes a domain separator, the network ID, and the
//!   channel contract address, preventing signatures from being reused across
//!   networks, channels, or confused with other signed payloads.
//! - The refund waiting period protects the recipient: it gives them time to
//!   withdraw using their latest commitment before the funder can reclaim
//!   funds.

#![no_std]
#[allow(unused_imports)]
use soroban_sdk::{assert_with_error, contract, contracterror, contractimpl, contracttype, symbol_short, token, xdr::ToXdr, Address, Bytes, BytesN, Env, Symbol};

pub mod event;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NegativeAmount = 1,
    NotClosed = 2,
    RefundWaitingPeriodNotElapsed = 3,
}

#[contracttype]
pub enum DataKey {
    Token,
    From,
    CommitmentKey,
    To,
    RefundWaitingPeriod,
    WithdrawnAmount,
    CloseEffectiveAtLedger,
}

#[contracttype]
pub struct Commitment {
    domain: Symbol,
    network: BytesN<32>,
    channel: Address,
    amount: i128,
}

impl Commitment {
    pub fn new(channel: Address, amount: i128) -> Self {
        let network = channel.env().ledger().network_id();
        Commitment {
            domain: symbol_short!("chancmmt"),
            network,
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
    ///   signatures. See `prepare_commitment` for details on
    ///   commitments.
    /// - `to`: The recipient who can `withdraw` funds using signed
    ///   commitments.
    /// - `amount`: The initial deposit amount.
    /// - `refund_waiting_period`: The number of ledgers the recipient has to
    ///   withdraw after `close` is called, before `refund`
    ///   becomes available. This value should be large enough to give the
    ///   recipient time to observe a close event and submit a withdrawal,
    ///   otherwise the recipient may not accept the channel. However, it
    ///   should not be so large that the funder cannot reclaim funds in a
    ///   timely manner. Setting zero or a very low number results in
    ///   near-immediate refunds, which is almost certainly not useful for
    ///   either participant.
    ///
    /// Callable by the deployer.
    ///
    /// # Auth
    /// - `from`: required if amount > 0.
    pub fn __constructor(env: &Env, token: Address, from: Address, commitment_key: BytesN<32>, to: Address, amount: i128, refund_waiting_period: u32) {
        assert_with_error!(env, amount >= 0, Error::NegativeAmount);

        // Store channel configuration.
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::From, &from);
        env.storage().instance().set(&DataKey::CommitmentKey, &commitment_key);
        env.storage().instance().set(&DataKey::To, &to);
        env.storage().instance().set(&DataKey::RefundWaitingPeriod, &refund_waiting_period);

        // Deposit initial funds.
        Self::top_up(env, amount);

        env.events().publish_event(&event::Open {
            from,
            commitment_key,
            to,
            token,
            amount,
            refund_waiting_period,
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
        assert_with_error!(env, amount >= 0, Error::NegativeAmount);
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

    /// Returns the refund waiting period in ledgers.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn refund_waiting_period(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::RefundWaitingPeriod).unwrap()
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
        env.storage().instance().get(&DataKey::WithdrawnAmount).unwrap_or(0)
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
    /// along with the amount, can be passed to `withdraw` by the
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
            env.storage().instance().set(&DataKey::WithdrawnAmount, &amount);
            Self::token_client(env).transfer(&env.current_contract_address(), &to, &payout);
            env.events().publish_event(&event::Withdraw { to, amount: payout });
        }
    }

    /// Close the channel, effective after a waiting period. The recipient can
    /// still withdraw during the waiting period. After the close is effective,
    /// the funder can call refund to reclaim the remaining balance.
    ///
    /// **Important:** The recipient should withdraw funds using `withdraw`
    /// whenever they see a [`event::Close`], before the close becomes effective.
    /// After the close is effective the funder can refund the remaining balance.
    /// The recipient can still withdraw even after the close is effective, up
    /// until `refund` is called.
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
        let refund_waiting_period = Self::refund_waiting_period(env);
        let effective_at_ledger = env.ledger().sequence().saturating_add(refund_waiting_period);
        env.storage().instance().set(&DataKey::CloseEffectiveAtLedger, &effective_at_ledger);

        env.events().publish_event(&event::Close { effective_at_ledger });
    }

    /// Refund the remaining balance to the funder after the close is effective.
    ///
    /// Can be called multiple times. This is useful if the funder accidentally
    /// deposits additional funds after closing — they can call refund again to
    /// reclaim the additional balance.
    ///
    /// Callable by the funder (from), after the close effective_at_ledger has
    /// been reached.
    ///
    /// # Auth
    /// - `from`: required.
    pub fn refund(env: &Env) -> Result<(), Error> {
        // Verify the close is effective.
        let effective_at_ledger: u32 = env.storage().instance().get(&DataKey::CloseEffectiveAtLedger).ok_or(Error::NotClosed)?;
        if env.ledger().sequence() < effective_at_ledger {
            return Err(Error::RefundWaitingPeriodNotElapsed);
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
