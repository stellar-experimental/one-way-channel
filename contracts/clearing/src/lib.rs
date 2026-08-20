//! # Clearing
//!
//! An optimistic payment clearing contract for Soroban (Stellar),
//! implementing the settlement-chain side of the Bajillion protocol
//! described in [Keep the Change](https://commonware.xyz/blogs/clearing).
//!
//! Bajillion is an optimistic clearing protocol for many-to-many payments at
//! massive scale. Payments flow off-chain through a non-custodial operator
//! selected by the sender. At each settlement the epoch's activity becomes a
//! few-kilobyte commitment: one row per changed account, regardless of how
//! many payments changed it. For a given set of accounts, one payment or a
//! bajillion costs the same to settle.
//!
//! > [!WARNING]
//! > **The contracts in this repository have not been audited.**
//!
//! ## Participants
//!
//! - **Accounts**: Identified by a registry index and an ed25519 key bound in
//!   the account's state leaf. Accounts sign debits (payments), exits
//!   (withdrawals), and unwind claims with this key.
//! - **Operator**: Serves payments off-chain, countersigning each accepted
//!   payment with a receipt, and builds epoch closes. The operator is
//!   non-custodial: funds never leave the contract except to released
//!   withdrawals and unwind claims.
//! - **Validators**: A fixed committee that exhaustively verifies each
//!   close's public corpus off-chain and signs its header. The contract
//!   accepts a close only with a quorum certificate.
//! - **Challengers**: Anyone holding a signed payment pair (typically payers,
//!   recipients, or watchtowers acting for them) can submit a one-shot
//!   challenge that proves a close contradicts a retained receipt.
//!
//! ## The clearing model
//!
//! Each account's persistent state is a leaf
//! `(active, balance, credit, debit, key, receipts)` in a fixed-capacity
//! Merkle registry. The registry's root is the **state root**. An epoch `e`
//! starts from `StateRoot_e`, fixes its boundary (deposits and user-signed
//! withdrawals), accepts payments off-chain, and closes by committing:
//!
//! - One **row** per changed account, strictly sorted by account index,
//!   carrying the account's opening and closing leaves, its terminal
//!   outgoing pair when it sent, the Merkle root of its receive-shard tips
//!   (the **credit root**), and a running prefix total over the sorted rows.
//! - A **change root**: a Merkle root binding the exact row count and every
//!   row in order.
//! - A **header** `(StateRoot_e, ChangeRoot_e, StateRoot_e+1, D_e, C_e, F_e,
//!   W_e, ...)` signed by a quorum of validators.
//!
//! The contract verifies the certificate, opens the terminal row against the
//! change root to check the header's totals, checks the chain-sealed boundary
//! (`F_e` and `W_e` must consume exactly the deposit and exit records the
//! contract recorded), and enforces the conservation laws:
//!
//! ```text
//! D_e = C_e                        (gross debits equal gross credits)
//! L_e+1 = L_e + F_e - W_e          (liability changes only by boundary flows)
//! ```
//!
//! Everything else — per-row balance equations, prefix continuity, the
//! paired sparse witness that recomputes both state roots, credit-root
//! reconstruction — is verified off-chain by the validator committee before
//! it signs. The complete public corpus must remain retrievable through the
//! challenge deadline.
//!
//! ## Off-chain payments
//!
//! To send `x > 0`, the payer signs the exact next cumulative debit and the
//! operator accepts by advancing one of the recipient's receive shards and
//! countersigning a receipt:
//!
//! ```text
//! S = Sign_a(epoch, a -> b: x, D_a + x)
//! R = Sign_op(epoch, b, shard, x, TxId(S), (G + x, J + 1))
//! ```
//!
//! The matching pair `(S, R)` is the accepted payment and the
//! preconfirmation, and doubles as the evidence that holds the close honest.
//! `TxId(S)` is the SHA-256 of the XDR serialized send body. Receive shards
//! let a hot recipient's incoming path scale in parallel: payments assigned
//! to different shards never contend, and one terminal tip per shard
//! represents any number of payments.
//!
//! All signed payloads are XDR serialized structs carrying a domain
//! separator, the network id, and this contract's address, preventing reuse
//! across networks, deployments, or payload kinds. See
//! [`Contract::prepare_send`], [`Contract::prepare_receipt`],
//! [`Contract::prepare_exit`], and [`Contract::prepare_claim`].
//!
//! ## The unavoidable challenge
//!
//! A validity proof over the public corpus cannot prove the nonexistence of
//! an additional privately delivered receipt, so every close waits out a
//! challenge window before finalizing. Through the inclusive deadline any
//! holder may submit one of four bounded, non-interactive contradictions:
//!
//! | # | Challenge | Function | Contradiction |
//! |---|---|---|---|
//! | 1 | Payer debit | `challenge_debit` | A matching pair carries a debit above the account's public debit marker. |
//! | 1 | Payer debit | `challenge_debit_body` | A matching pair carries the same debit as the row's terminal pair but a different send or receipt body. |
//! | 2 | Higher shard tip | `challenge_tip` | A retained receipt strictly exceeds the shard tip bound in the row's credit root. |
//! | 2 | Higher shard tip | `challenge_tip_absent` | A retained receipt names a shard the credit root proves absent (tip `(0, 0)`). |
//! | 2 | Higher shard tip | `challenge_tip_no_row` | A retained receipt credits an account the close proves rowless (tip `(0, 0)`). |
//! | 3 | Receipt range | `challenge_range` | Two receipts in one shard whose credits cannot bracket the payments between them. |
//! | 4 | Receipt fork | `challenge_fork` | Two distinct receipt bodies reuse a receipt index or acknowledge one send differently. |
//!
//! A successful challenge blocks the contested close and every pending
//! descendant from finalizing (the queue is truncated). Earlier pending
//! closes keep their ordinary challenge windows. The operator may submit a
//! corrected close for the invalidated epoch.
//!
//! ## Exits and the deadline
//!
//! Every account holds a unilateral exit: a signed withdrawal
//! `Q = Sign_a(root, destination, amount, full_close, deadline)` queued
//! directly on-chain with a Merkle proof of affordability against the
//! finalized root. The operator neither submits nor approves it. A close
//! consumes queued exits in order as part of its chain-sealed boundary, and
//! once that close finalizes anyone can `release` the payment to the signed
//! destination.
//!
//! If an exit is still unreleased when its deadline passes, the first call
//! to `freeze` permanently stops new deposits, exits, and closes. Pending
//! closes still resolve from the front — each finalizes when its window
//! closes, or falls to a challenge — and terminal unwind opens against the
//! last finalized root:
//!
//! - `unwind_exit` pays queued, unconsumed exits to their signed
//!   destinations, capped by the account's proven balance.
//! - `unwind_claim` pays an account's remaining balance to a destination
//!   signed by the account key, against one Merkle proof.
//! - `unwind_deposit` refunds deposits no finalized close consumed to their
//!   depositors.
//!
//! Custody never leaves the chain: withdrawals stay inside until their own
//! close finalizes at the queue front, so the operator can stop serving
//! payments, but it cannot take funds or send them without authorization.
//!
//! ## Functions
//!
//! ### Lifecycle
//!
//! | Function | Description |
//! |---|---|
//! | `__constructor` | Deploy with a token, operator key, validator committee, quorum, registry depth, challenge window, and minimum exit delay. |
//! | `deposit` | Deposit tokens for an account key. Recorded as a boundary record for the next close. |
//! | `exit` | Queue a signed withdrawal with a proof of affordability against the finalized root. |
//! | `submit` | Submit a certified close for the next epoch into the pending queue. |
//! | `finalize` | Finalize the front of the pending queue after its challenge window. |
//! | `release` | Pay a withdrawal consumed by a finalized close to its signed destination. |
//! | `freeze` | Permanently freeze new work after an exit deadline is breached. |
//!
//! ### Challenges
//!
//! | Function | Description |
//! |---|---|
//! | `challenge_debit` | Prove a retained pair's debit exceeds the public debit marker. |
//! | `challenge_debit_body` | Prove a retained pair differs from the terminal pair at the same debit. |
//! | `challenge_tip` | Prove a retained receipt exceeds a shard tip bound in the close. |
//! | `challenge_tip_absent` | Prove a retained receipt names a shard the close binds as absent. |
//! | `challenge_tip_no_row` | Prove a retained receipt credits an account with no row in the close. |
//! | `challenge_range` | Prove two receipts in one shard are mutually inconsistent. |
//! | `challenge_fork` | Prove the operator signed two forking receipts. |
//!
//! ### Terminal unwind
//!
//! | Function | Description |
//! |---|---|
//! | `unwind_exit` | Pay a queued, unconsumed exit against the last finalized root. |
//! | `unwind_claim` | Claim an account's remaining balance with a Merkle proof and a signed destination. |
//! | `unwind_deposit` | Refund a deposit no finalized close consumed. |
//!
//! ### Helpers
//!
//! | Function | Description |
//! |---|---|
//! | `prepare_send` | Generate the send payload bytes a payer signs. |
//! | `prepare_receipt` | Generate the receipt payload bytes the operator signs. |
//! | `prepare_exit` | Generate the exit payload bytes an account signs. |
//! | `prepare_claim` | Generate the unwind claim payload bytes an account signs. |
//!
//! ### Getters
//!
//! | Function | Description |
//! |---|---|
//! | `token` | The custody token address. |
//! | `operator_key` | The operator's receipt signing key. |
//! | `validators` | The validator committee keys. |
//! | `quorum` | The certificate quorum size. |
//! | `registry_depth` | The account registry tree depth. |
//! | `challenge_window` | The challenge window in ledgers. |
//! | `min_exit_delay` | The minimum ledgers between queueing an exit and its deadline. |
//! | `frozen` | Whether new work is frozen. |
//! | `next_epoch` | The next epoch a close can be submitted for. |
//! | `finalized_epoch` | The number of finalized closes. |
//! | `finalized_root` | The last finalized state root (the genesis root before any close). |
//! | `finalized_liability` | The total balance the registry owes accounts at the finalized root. |
//! | `custody` | The token balance held by the contract. |
//! | `slot` | A pending or finalized close by epoch. |
//! | `deposit_count` / `deposit_record` | Deposit boundary records. |
//! | `exit_count` / `exit_record` | Exit boundary records. |
//! | `pending_exits` | The total queued, unpaid exit amount for a key. |
//! | `unwound` | The total already paid to a key during terminal unwind. |
//!
//! ## Trust model and deviations
//!
//! The contract trusts a quorum of the validator committee for the
//! correctness of everything it does not check itself (per-row balance
//! equations, registration validity, exit re-affordability at pending
//! roots). Retained receipts keep even a colluding operator and committee
//! from finalizing a close that drops or understates accepted payments, and
//! `freeze` plus terminal unwind guarantee recovery through the settlement
//! chain alone. Known simplifications relative to the blog post:
//!
//! - The validator committee is fixed at deployment; rotation is out of
//!   scope.
//! - A withdrawal releases exactly its signed amount. The `full_close` flag
//!   is carried in the signed payload and honored during terminal unwind
//!   (the exit drains the account's proven balance), but committee policy
//!   governs account deactivation inside closes.
//! - Receipts naming a key that never appears in the registry are not
//!   challengeable on-chain: the debit and tip challenges authenticate the
//!   recipient's key through its registry leaf. Registration completeness is
//!   part of what the committee attests.
//! - Data availability of the public corpus (rows, shard vectors, witness)
//!   through the challenge deadline is assumed, as in the blog post, via
//!   committee assignment and quorum intersection.

#![no_std]
use soroban_sdk::{assert_with_error, contract, contracterror, contractimpl, contracttype, symbol_short, token, xdr::ToXdr, Address, Bytes, BytesN, Env, Symbol, Vec};

pub mod event;
pub mod merkle;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    InvalidSetup = 1,
    NonPositiveAmount = 2,
    Overflow = 3,
    Frozen = 4,
    NotFrozen = 5,
    WrongEpoch = 6,
    WrongParentRoot = 7,
    ContextMismatch = 8,
    InvalidCertificate = 9,
    QuorumNotMet = 10,
    MissingTerminalRow = 11,
    InvalidOpening = 12,
    TotalsMismatch = 13,
    PaymentsNotConserved = 14,
    BoundaryMismatch = 15,
    NegativeLiability = 16,
    NoSuchSlot = 17,
    ChallengeWindowClosed = 18,
    ChallengeWindowOpen = 19,
    NothingPending = 20,
    PairMismatch = 21,
    NoContradiction = 22,
    KeyMismatch = 23,
    InvalidAbsence = 24,
    InsufficientBalance = 25,
    DeadlineTooSoon = 26,
    NoSuchRecord = 27,
    AlreadyPaid = 28,
    NotReleasable = 29,
    Releasable = 30,
    DeadlineNotReached = 31,
    AlreadyFrozen = 32,
    PendingSlotsRemain = 33,
}

#[contracttype]
pub enum DataKey {
    Token,
    OperatorKey,
    Validators,
    Quorum,
    ChallengeWindow,
    RegistryDepth,
    MinExitDelay,
    GenesisRoot,
    NextEpoch,
    FinalizedEpoch,
    Frozen,
    DepositCount,
    ExitCount,
    Slot(u64),
    Deposit(u32),
    Exit(u32),
    PendingExit(BytesN<32>),
    Unwound(BytesN<32>),
}

/// An account's persistent state: one leaf of the registry tree.
///
/// The leaf of an unregistered registry position is the sentinel with a zero
/// key, zero totals, and the activity flag unset; it hashes to the all-zero
/// digest.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountLeaf {
    /// The activity flag.
    pub active: bool,
    /// The balance `B_a`.
    pub balance: i128,
    /// The cumulative operator-promised credit `C_a`.
    pub credit: i128,
    /// The cumulative debit `D_a`.
    pub debit: i128,
    /// The ed25519 key that authorizes the account's sends, exits, and
    /// claims.
    pub key: BytesN<32>,
    /// The cumulative incoming receipt count.
    pub receipts: u64,
}

impl AccountLeaf {
    /// The sentinel leaf of an unregistered registry position.
    pub fn empty(env: &Env) -> Self {
        AccountLeaf {
            active: false,
            balance: 0,
            credit: 0,
            debit: 0,
            key: BytesN::from_array(env, &[0u8; 32]),
            receipts: 0,
        }
    }

    /// Whether this is the sentinel leaf of an unregistered position.
    pub fn is_empty(&self, env: &Env) -> bool {
        *self == Self::empty(env)
    }

    /// The Merkle digest of this leaf: the all-zero digest for the sentinel,
    /// `sha256(0x00 || xdr(leaf))` otherwise.
    pub fn digest(&self, env: &Env) -> BytesN<32> {
        if self.is_empty(env) {
            merkle::empty(env)
        } else {
            merkle::leaf(env, &self.clone().to_xdr(env))
        }
    }
}

/// The payload a payer signs to send a payment: the exact next cumulative
/// debit.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Send {
    /// The payment amount `x`.
    pub amount: i128,
    /// The clearing contract address.
    pub contract: Address,
    /// The payer's cumulative debit after this payment, `D_a + x`.
    pub debit: i128,
    /// The domain separator `clrsend`.
    pub domain: Symbol,
    /// The epoch the payment belongs to.
    pub epoch: u64,
    /// The payer's account key.
    pub from: BytesN<32>,
    /// The network id.
    pub network: BytesN<32>,
    /// The recipient's account key.
    pub to: BytesN<32>,
}

impl Send {
    pub fn new(contract: Address, from: BytesN<32>, to: BytesN<32>, amount: i128, debit: i128, epoch: u64) -> Self {
        let network = contract.env().ledger().network_id();
        Send {
            amount,
            contract,
            debit,
            domain: symbol_short!("clrsend"),
            epoch,
            from,
            network,
            to,
        }
    }

    /// The XDR serialized payload the payer signs.
    pub fn bytes(&self) -> Bytes {
        let env = self.contract.env();
        self.clone().to_xdr(env)
    }

    /// The transaction id of this send: the SHA-256 of the signed payload.
    pub fn txid(&self) -> BytesN<32> {
        let env = self.contract.env();
        merkle::sha256(env, &self.bytes())
    }
}

/// The payload the operator signs to accept a payment: the advance of one of
/// the recipient's receive shards.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    /// The payment amount `x`.
    pub amount: i128,
    /// The clearing contract address.
    pub contract: Address,
    /// The shard's receipt count after this payment, `J + 1`.
    pub count: u64,
    /// The shard's cumulative credit after this payment, `G + x`.
    pub credit: i128,
    /// The domain separator `clrrcpt`.
    pub domain: Symbol,
    /// The epoch the payment belongs to.
    pub epoch: u64,
    /// The network id.
    pub network: BytesN<32>,
    /// The recipient's account key.
    pub recipient: BytesN<32>,
    /// The receive shard index.
    pub shard: u32,
    /// The transaction id of the send this receipt acknowledges.
    pub txid: BytesN<32>,
}

impl Receipt {
    pub fn new(contract: Address, recipient: BytesN<32>, shard: u32, amount: i128, txid: BytesN<32>, credit: i128, count: u64, epoch: u64) -> Self {
        let network = contract.env().ledger().network_id();
        Receipt {
            amount,
            contract,
            count,
            credit,
            domain: symbol_short!("clrrcpt"),
            epoch,
            network,
            recipient,
            shard,
            txid,
        }
    }

    /// The XDR serialized payload the operator signs.
    pub fn bytes(&self) -> Bytes {
        let env = self.contract.env();
        self.clone().to_xdr(env)
    }
}

/// A matching send and receipt: the accepted payment, the preconfirmation,
/// and the settlement evidence.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pair {
    /// The receipt body.
    pub receipt: Receipt,
    /// The operator's signature over the receipt payload.
    pub receipt_sig: BytesN<64>,
    /// The send body.
    pub send: Send,
    /// The payer's signature over the send payload.
    pub send_sig: BytesN<64>,
}

/// The terminal outgoing pair of a row: present exactly when the account
/// sent during the epoch.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Out {
    /// The account did not send.
    Absent,
    /// The account's terminal outgoing pair: the accepted payment at the
    /// account's closing debit.
    Terminal(Pair),
}

/// The terminal state of one receive shard: its cumulative credit and
/// receipt count. Shard tips are the leaves of a row's credit root, at the
/// position of their shard index.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShardTip {
    /// The shard's receipt count `J`.
    pub count: u64,
    /// The shard's cumulative credit `G`.
    pub credit: i128,
}

impl ShardTip {
    /// The Merkle digest of this tip.
    pub fn digest(&self, env: &Env) -> BytesN<32> {
        merkle::leaf(env, &self.clone().to_xdr(env))
    }
}

/// Running totals over the strictly sorted rows of a close, through and
/// including the row that carries the prefix. The terminal row's prefix
/// alone carries the epoch's totals.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prefix {
    /// The credit deltas `sum(c_a)`.
    pub credits: i128,
    /// The debit deltas `sum(d_a)`.
    pub debits: i128,
    /// The deposits `sum(f_a)`.
    pub deposits: i128,
    /// The receive shard head counts `sum(h_a)`.
    pub shards: u64,
    /// The withdrawal record counts `sum(chi_a)`.
    pub withdrawal_records: u32,
    /// The withdrawals `sum(w_a)`.
    pub withdrawals: i128,
}

/// One row per changed account in a close.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    /// The account's registry index.
    pub account: u32,
    /// The account's closing state `X^1`.
    pub closing: AccountLeaf,
    /// The Merkle root binding the account's receive shard tips and their
    /// exact count.
    pub credit_root: BytesN<32>,
    /// The account's opening state `X^0`.
    pub opening: AccountLeaf,
    /// The account's terminal outgoing pair, present when the account sent.
    pub out: Out,
    /// The running totals through this row.
    pub prefix: Prefix,
}

impl Row {
    /// The Merkle digest of this row.
    pub fn digest(&self, env: &Env) -> BytesN<32> {
        merkle::leaf(env, &self.clone().to_xdr(env))
    }
}

/// The header of a close: the only part of a close the chain retains. The
/// validator committee signs the XDR serialized header.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    /// The Merkle root binding the close's rows and their exact count.
    pub change_root: BytesN<32>,
    /// The clearing contract address.
    pub contract: Address,
    /// The gross credit `C_e`.
    pub credits: i128,
    /// The gross debit `D_e`.
    pub debits: i128,
    /// The consumed deposit total `F_e`.
    pub deposits: i128,
    /// The deposit records consumed through this close: records
    /// `[parent.deposits_to, deposits_to)`.
    pub deposits_to: u32,
    /// The domain separator `clrhead`.
    pub domain: Symbol,
    /// The epoch this close settles.
    pub epoch: u64,
    /// The network id.
    pub network: BytesN<32>,
    /// The row count `A_e`.
    pub rows: u32,
    /// The receive shard head count `H_e`.
    pub shards: u64,
    /// The state root the close opens from, `StateRoot_e`.
    pub state_root: BytesN<32>,
    /// The state root the close transitions to, `StateRoot_e+1`.
    pub state_root_after: BytesN<32>,
    /// The consumed withdrawal record count `chi_e`.
    pub withdrawal_records: u32,
    /// The consumed withdrawal total `W_e`.
    pub withdrawals: i128,
    /// The exit records consumed through this close: records
    /// `[parent.withdrawals_to, withdrawals_to)`.
    pub withdrawals_to: u32,
}

impl Header {
    /// The XDR serialized payload validators sign.
    pub fn bytes(&self) -> Bytes {
        let env = self.contract.env();
        self.clone().to_xdr(env)
    }
}

/// One validator's signature over a header, by committee index.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    /// The validator's index in the committee.
    pub index: u32,
    /// The validator's ed25519 signature over the header payload.
    pub signature: BytesN<64>,
}

/// A submitted close in the pending queue (or, once finalized, the record of
/// a finalized close).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Slot {
    /// The ledger sequence number through which the close can be challenged
    /// (inclusive).
    pub deadline: u32,
    /// The close's header.
    pub header: Header,
    /// The registry's total account balance after the close.
    pub liability_after: i128,
}

/// The payload an account signs to queue a unilateral exit.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Exit {
    /// The amount to withdraw.
    pub amount: i128,
    /// The clearing contract address.
    pub contract: Address,
    /// The absolute ledger sequence number after which an unreleased exit
    /// freezes the system.
    pub deadline: u32,
    /// The destination address paid.
    pub destination: Address,
    /// The domain separator `clrexit`.
    pub domain: Symbol,
    /// Whether the account asks to be closed entirely. During terminal
    /// unwind a full-close exit drains the account's proven balance.
    pub full_close: bool,
    /// The network id.
    pub network: BytesN<32>,
    /// The finalized state root the exit was signed against.
    pub root: BytesN<32>,
}

impl Exit {
    pub fn new(contract: Address, destination: Address, amount: i128, full_close: bool, deadline: u32, root: BytesN<32>) -> Self {
        let network = contract.env().ledger().network_id();
        Exit {
            amount,
            contract,
            deadline,
            destination,
            domain: symbol_short!("clrexit"),
            full_close,
            network,
            root,
        }
    }

    /// The XDR serialized payload the account signs.
    pub fn bytes(&self) -> Bytes {
        let env = self.contract.env();
        self.clone().to_xdr(env)
    }
}

/// The payload an account signs to claim its remaining balance during
/// terminal unwind.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claim {
    /// The clearing contract address.
    pub contract: Address,
    /// The destination address paid.
    pub destination: Address,
    /// The domain separator `clrclaim`.
    pub domain: Symbol,
    /// The network id.
    pub network: BytesN<32>,
    /// The state root the unwind resolves against.
    pub root: BytesN<32>,
}

impl Claim {
    pub fn new(contract: Address, destination: Address, root: BytesN<32>) -> Self {
        let network = contract.env().ledger().network_id();
        Claim {
            contract,
            destination,
            domain: symbol_short!("clrclaim"),
            network,
            root,
        }
    }

    /// The XDR serialized payload the account signs.
    pub fn bytes(&self) -> Bytes {
        let env = self.contract.env();
        self.clone().to_xdr(env)
    }
}

/// A recorded deposit: part of the chain-sealed boundary a close consumes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositRecord {
    /// The amount deposited.
    pub amount: i128,
    /// The running total of all deposits through and including this record.
    pub cumulative: i128,
    /// The address the tokens were transferred from, refunded during
    /// terminal unwind if no finalized close consumes the record.
    pub depositor: Address,
    /// The account key the deposit is for.
    pub key: BytesN<32>,
    /// Whether the record was refunded during terminal unwind.
    pub refunded: bool,
}

/// A queued exit: part of the chain-sealed boundary a close consumes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitRecord {
    /// The amount to withdraw.
    pub amount: i128,
    /// The running total of all exit amounts through and including this
    /// record.
    pub cumulative: i128,
    /// The ledger sequence number after which an unreleased exit freezes the
    /// system.
    pub deadline: u32,
    /// The destination address paid.
    pub destination: Address,
    /// Whether the account asked to be closed entirely.
    pub full_close: bool,
    /// The account key the exit debits.
    pub key: BytesN<32>,
    /// Whether the exit was paid, via `release` or `unwind_exit`.
    pub paid: bool,
}

/// A proof that a close contains no row for an account index. Rows are
/// strictly sorted by account, and the change root binds the exact row
/// count, so absence is proven by the close being empty, by a boundary row,
/// or by two adjacent rows that straddle the index.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RowAbsence {
    /// The close has no rows at all.
    Empty,
    /// The first row's account is greater than the index. Carries the row at
    /// position `0` and its opening.
    Before(Row, Vec<BytesN<32>>),
    /// The last row's account is less than the index. Carries the row at
    /// position `rows - 1` and its opening.
    After(Row, Vec<BytesN<32>>),
    /// Two adjacent rows straddle the index. Carries the position of the
    /// left row, the left row and its opening, and the right row (at the
    /// next position) and its opening.
    Between(u32, Row, Vec<BytesN<32>>, Row, Vec<BytesN<32>>),
}

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    /// Deploy the clearing system.
    ///
    /// - `token`: The SEP-41 token held in custody.
    /// - `operator_key`: The ed25519 key the operator signs receipts with.
    /// - `validators`: The validator committee's ed25519 keys, by index.
    /// - `quorum`: The number of validator signatures a close certificate
    ///   requires. Must satisfy `1 <= quorum <= validators.len()`. For the
    ///   protocol's quorum intersection argument with `n` validators and `f`
    ///   tolerated faults, use `quorum = 2f + 1` with `n = 3f + 1`.
    /// - `registry_depth`: The depth of the account registry tree; capacity
    ///   is `2^registry_depth` accounts. Must satisfy
    ///   `1 <= registry_depth <= 30`.
    /// - `challenge_window`: The number of ledgers a submitted close remains
    ///   challengeable. Should be long enough for receipt holders to observe
    ///   the close's public corpus and submit a challenge.
    /// - `min_exit_delay`: The minimum number of ledgers between queueing an
    ///   exit and its deadline, giving the operator time to consume the exit
    ///   in an orderly close before it can freeze the system.
    ///
    /// The registry starts empty: the genesis state root commits `2^depth`
    /// empty positions, and the genesis liability is zero.
    ///
    /// Callable by the deployer.
    ///
    /// # Auth
    /// None.
    pub fn __constructor(env: &Env, token: Address, operator_key: BytesN<32>, validators: Vec<BytesN<32>>, quorum: u32, registry_depth: u32, challenge_window: u32, min_exit_delay: u32) {
        assert_with_error!(env, !validators.is_empty(), Error::InvalidSetup);
        assert_with_error!(env, quorum >= 1 && quorum <= validators.len(), Error::InvalidSetup);
        assert_with_error!(env, registry_depth >= 1 && registry_depth <= 30, Error::InvalidSetup);

        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::OperatorKey, &operator_key);
        env.storage().instance().set(&DataKey::Validators, &validators);
        env.storage().instance().set(&DataKey::Quorum, &quorum);
        env.storage().instance().set(&DataKey::RegistryDepth, &registry_depth);
        env.storage().instance().set(&DataKey::ChallengeWindow, &challenge_window);
        env.storage().instance().set(&DataKey::MinExitDelay, &min_exit_delay);
        env.storage().instance().set(&DataKey::GenesisRoot, &merkle::empty_root(env, registry_depth));

        env.events().publish_event(&event::Setup {
            token,
            operator_key,
            validators: validators.len(),
            quorum,
            registry_depth,
            challenge_window,
            min_exit_delay,
        });
    }

    /// Deposit tokens for an account key.
    ///
    /// The transfer happens immediately; the deposit becomes spendable once
    /// a close consumes the record and credits the key's account (the
    /// operator registers the key at a free registry index if it has no
    /// account yet). Deposits are consumed strictly in order. If the system
    /// freezes before any finalized close consumes the record, the depositor
    /// is refunded via `unwind_deposit`.
    ///
    /// Returns the deposit record's sequence number.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// - `depositor`: required.
    pub fn deposit(env: &Env, depositor: Address, key: BytesN<32>, amount: i128) -> Result<u32, Error> {
        if Self::frozen(env) {
            return Err(Error::Frozen);
        }
        if amount <= 0 {
            return Err(Error::NonPositiveAmount);
        }
        depositor.require_auth();

        let sequence: u32 = Self::deposit_count(env);
        let cumulative = Self::deposit_cumulative(env, sequence).checked_add(amount).ok_or(Error::Overflow)?;
        let record = DepositRecord {
            amount,
            cumulative,
            depositor: depositor.clone(),
            key: key.clone(),
            refunded: false,
        };
        env.storage().persistent().set(&DataKey::Deposit(sequence), &record);
        env.storage().instance().set(&DataKey::DepositCount, &(sequence + 1));

        Self::token_client(env).transfer(&depositor, &env.current_contract_address(), &amount);

        env.events().publish_event(&event::Deposit { sequence, depositor, key, amount });
        Ok(sequence)
    }

    /// Queue a unilateral exit: a withdrawal signed by the account key, not
    /// operator-controlled.
    ///
    /// The signed payload names the current finalized state root, the
    /// destination, the amount, the full-close flag, and the absolute
    /// deadline; see `prepare_exit`. The caller proves affordability by
    /// opening the account's leaf against the finalized root: the leaf's
    /// balance must cover this exit plus every other queued, unpaid exit for
    /// the key. The deadline must be at least `min_exit_delay` ledgers away.
    ///
    /// The operator is expected to consume the record in a close. Once that
    /// close finalizes, anyone can `release` the payment. If the record is
    /// still unreleased when the deadline passes, anyone can `freeze` the
    /// system.
    ///
    /// Returns the exit record's sequence number.
    ///
    /// Callable by anyone holding the signed payload.
    ///
    /// # Auth
    /// - Exit signature serves as account key authorization.
    pub fn exit(env: &Env, destination: Address, amount: i128, full_close: bool, deadline: u32, sig: BytesN<64>, account: u32, leaf: AccountLeaf, proof: Vec<BytesN<32>>) -> Result<u32, Error> {
        if Self::frozen(env) {
            return Err(Error::Frozen);
        }
        if amount <= 0 {
            return Err(Error::NonPositiveAmount);
        }
        if deadline < env.ledger().sequence().saturating_add(Self::min_exit_delay(env)) {
            return Err(Error::DeadlineTooSoon);
        }

        let root = Self::finalized_root(env);
        Self::verify_state_opening(env, &root, account, &leaf, &proof)?;
        if leaf.is_empty(env) {
            return Err(Error::KeyMismatch);
        }
        let body = Exit::new(env.current_contract_address(), destination.clone(), amount, full_close, deadline, root);
        env.crypto().ed25519_verify(&leaf.key, &body.bytes(), &sig);

        let pending = Self::pending_exits(env, leaf.key.clone());
        let reserved = pending.checked_add(amount).ok_or(Error::Overflow)?;
        if reserved > leaf.balance {
            return Err(Error::InsufficientBalance);
        }

        let sequence: u32 = Self::exit_count(env);
        let cumulative = Self::exit_cumulative(env, sequence).checked_add(amount).ok_or(Error::Overflow)?;
        let record = ExitRecord {
            amount,
            cumulative,
            deadline,
            destination: destination.clone(),
            full_close,
            key: leaf.key.clone(),
            paid: false,
        };
        env.storage().persistent().set(&DataKey::Exit(sequence), &record);
        env.storage().instance().set(&DataKey::ExitCount, &(sequence + 1));
        env.storage().persistent().set(&DataKey::PendingExit(leaf.key.clone()), &reserved);

        env.events().publish_event(&event::ExitQueued {
            sequence,
            key: leaf.key,
            destination,
            amount,
            full_close,
            deadline,
        });
        Ok(sequence)
    }

    /// Submit a certified close for the next epoch.
    ///
    /// The close enters the pending queue and remains challengeable through
    /// its deadline. The contract verifies:
    ///
    /// - The header's domain, network, contract address, and epoch, and that
    ///   its opening state root chains from the previous close (or the
    ///   genesis root).
    /// - A quorum certificate: at least `quorum` valid validator signatures
    ///   over the header payload, in strictly increasing committee index
    ///   order.
    /// - The terminal row: its Merkle opening at position `rows - 1` under
    ///   the change root, and that its prefix equals the header's totals.
    ///   A close with zero rows must carry zero totals and leave the state
    ///   root unchanged.
    /// - Payment conservation `D_e = C_e`.
    /// - The chain-sealed boundary: the close consumes deposit records
    ///   `[parent.deposits_to, deposits_to)` and exit records
    ///   `[parent.withdrawals_to, withdrawals_to)`, and `F_e`, `W_e`, and
    ///   `chi_e` must equal the recorded sums and count.
    /// - Liability conservation: `L_e+1 = L_e + F_e - W_e >= 0`.
    ///
    /// Everything else is attested by the certificate.
    ///
    /// Callable by anyone holding a certified close.
    ///
    /// # Auth
    /// - Certificate serves as validator committee authorization.
    pub fn submit(env: &Env, header: Header, certificate: Vec<Signature>, terminal_row: Option<Row>, terminal_proof: Vec<BytesN<32>>) -> Result<(), Error> {
        if Self::frozen(env) {
            return Err(Error::Frozen);
        }

        // Context.
        if header.domain != symbol_short!("clrhead") || header.network != env.ledger().network_id() || header.contract != env.current_contract_address() {
            return Err(Error::ContextMismatch);
        }
        let next = Self::next_epoch(env);
        if header.epoch != next {
            return Err(Error::WrongEpoch);
        }
        if header.state_root != Self::head_root(env) {
            return Err(Error::WrongParentRoot);
        }

        // Certificate.
        let validators: Vec<BytesN<32>> = env.storage().instance().get(&DataKey::Validators).unwrap();
        if certificate.len() < Self::quorum(env) {
            return Err(Error::QuorumNotMet);
        }
        let payload = header.bytes();
        let mut previous: Option<u32> = None;
        for entry in certificate.iter() {
            if previous.is_some_and(|p| entry.index <= p) {
                return Err(Error::InvalidCertificate);
            }
            let key = validators.get(entry.index).ok_or(Error::InvalidCertificate)?;
            env.crypto().ed25519_verify(&key, &payload, &entry.signature);
            previous = Some(entry.index);
        }

        // Totals and the terminal row.
        if header.debits < 0 || header.credits < 0 || header.deposits < 0 || header.withdrawals < 0 {
            return Err(Error::TotalsMismatch);
        }
        if header.debits != header.credits {
            return Err(Error::PaymentsNotConserved);
        }
        if header.rows == 0 {
            if terminal_row.is_some() || !terminal_proof.is_empty() {
                return Err(Error::InvalidOpening);
            }
            if header.debits != 0 || header.deposits != 0 || header.withdrawals != 0 || header.withdrawal_records != 0 || header.shards != 0 {
                return Err(Error::TotalsMismatch);
            }
            if header.change_root != merkle::counted(env, 0, &merkle::empty(env)) || header.state_root_after != header.state_root {
                return Err(Error::InvalidOpening);
            }
        } else {
            let row = terminal_row.ok_or(Error::MissingTerminalRow)?;
            if !merkle::verify_counted(env, &header.change_root, header.rows, header.rows - 1, &row.digest(env), &terminal_proof) {
                return Err(Error::InvalidOpening);
            }
            let p = &row.prefix;
            if p.debits != header.debits
                || p.credits != header.credits
                || p.deposits != header.deposits
                || p.withdrawals != header.withdrawals
                || p.withdrawal_records != header.withdrawal_records
                || p.shards != header.shards
            {
                return Err(Error::TotalsMismatch);
            }
        }

        // Chain-sealed boundary.
        let (deposits_from, withdrawals_from) = Self::boundary(env);
        if header.deposits_to < deposits_from || header.deposits_to > Self::deposit_count(env) {
            return Err(Error::BoundaryMismatch);
        }
        if header.deposits != Self::deposit_cumulative(env, header.deposits_to) - Self::deposit_cumulative(env, deposits_from) {
            return Err(Error::BoundaryMismatch);
        }
        if header.withdrawals_to < withdrawals_from || header.withdrawals_to > Self::exit_count(env) {
            return Err(Error::BoundaryMismatch);
        }
        if header.withdrawal_records != header.withdrawals_to - withdrawals_from {
            return Err(Error::BoundaryMismatch);
        }
        if header.withdrawals != Self::exit_cumulative(env, header.withdrawals_to) - Self::exit_cumulative(env, withdrawals_from) {
            return Err(Error::BoundaryMismatch);
        }

        // Liability conservation.
        let liability_after = Self::head_liability(env)
            .checked_add(header.deposits)
            .and_then(|l| l.checked_sub(header.withdrawals))
            .ok_or(Error::Overflow)?;
        if liability_after < 0 {
            return Err(Error::NegativeLiability);
        }

        // Queue the close.
        let deadline = env.ledger().sequence().saturating_add(Self::challenge_window(env));
        let state_root_after = header.state_root_after.clone();
        let slot = Slot { deadline, header, liability_after };
        env.storage().persistent().set(&DataKey::Slot(next), &slot);
        env.storage().instance().set(&DataKey::NextEpoch, &(next + 1));

        env.events().publish_event(&event::Submit {
            epoch: next,
            state_root_after,
            deadline,
        });
        Ok(())
    }

    /// Finalize the close at the front of the pending queue after its
    /// challenge window has passed.
    ///
    /// Pending closes finalize strictly in order. Withdrawals consumed by
    /// the finalized close become releasable.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn finalize(env: &Env) -> Result<(), Error> {
        let finalized = Self::finalized_epoch(env);
        if finalized >= Self::next_epoch(env) {
            return Err(Error::NothingPending);
        }
        let slot = Self::slot(env, finalized).unwrap();
        if env.ledger().sequence() <= slot.deadline {
            return Err(Error::ChallengeWindowOpen);
        }
        env.storage().instance().set(&DataKey::FinalizedEpoch, &(finalized + 1));

        env.events().publish_event(&event::Finalize {
            epoch: finalized,
            state_root: slot.header.state_root_after,
            liability: slot.liability_after,
        });
        Ok(())
    }

    /// Release a withdrawal consumed by a finalized close, paying its signed
    /// destination.
    ///
    /// Callable by anyone, including after a freeze.
    ///
    /// # Auth
    /// None.
    pub fn release(env: &Env, sequence: u32) -> Result<(), Error> {
        let mut record: ExitRecord = env.storage().persistent().get(&DataKey::Exit(sequence)).ok_or(Error::NoSuchRecord)?;
        if sequence >= Self::released_boundary(env) {
            return Err(Error::NotReleasable);
        }
        if record.paid {
            return Err(Error::AlreadyPaid);
        }
        record.paid = true;
        env.storage().persistent().set(&DataKey::Exit(sequence), &record);
        Self::pending_exit_sub(env, &record.key, record.amount);

        Self::token_client(env).transfer(&env.current_contract_address(), &record.destination, &record.amount);

        env.events().publish_event(&event::Release {
            sequence,
            destination: record.destination,
            amount: record.amount,
        });
        Ok(())
    }

    /// Freeze the system: an exit deadline has passed with the exit not yet
    /// covered by a finalized close.
    ///
    /// The first call to observe a breached deadline permanently freezes new
    /// work: no more deposits, exits, or close submissions. Pending closes
    /// still resolve from the front — finalizing after their challenge
    /// windows or falling to challenges — and once none remain, terminal
    /// unwind opens against the last finalized root.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn freeze(env: &Env, sequence: u32) -> Result<(), Error> {
        if Self::frozen(env) {
            return Err(Error::AlreadyFrozen);
        }
        let record: ExitRecord = env.storage().persistent().get(&DataKey::Exit(sequence)).ok_or(Error::NoSuchRecord)?;
        if sequence < Self::released_boundary(env) {
            // Covered by a finalized close: the payment is already
            // releasable permissionlessly, so the deadline cannot freeze.
            return Err(Error::Releasable);
        }
        if env.ledger().sequence() < record.deadline {
            return Err(Error::DeadlineNotReached);
        }
        env.storage().instance().set(&DataKey::Frozen, &true);
        env.events().publish_event(&event::Freeze { sequence });
        Ok(())
    }

    /// Challenge 1a — payer debit contradiction.
    ///
    /// A matching acknowledged pair carries a debit above the public debit
    /// marker. The challenger opens the payer's account leaf against the
    /// challenged close's closing state root and presents a retained pair
    /// whose send debit strictly exceeds the leaf's cumulative debit. A bare
    /// payer request is insufficient: the receipt proves operator
    /// acknowledgement.
    ///
    /// Callable by anyone, through the close's challenge deadline.
    ///
    /// # Auth
    /// None.
    pub fn challenge_debit(env: &Env, epoch: u64, pair: Pair, account: u32, leaf: AccountLeaf, proof: Vec<BytesN<32>>) -> Result<(), Error> {
        let slot = Self::require_challengeable(env, epoch)?;
        Self::verify_pair(env, epoch, &pair)?;
        Self::verify_state_opening(env, &slot.header.state_root_after, account, &leaf, &proof)?;
        if leaf.key != pair.send.from {
            return Err(Error::KeyMismatch);
        }
        if pair.send.debit <= leaf.debit {
            return Err(Error::NoContradiction);
        }
        Self::invalidate(env, epoch, 1);
        Ok(())
    }

    /// Challenge 1b — payer debit contradiction at the terminal pair.
    ///
    /// A matching acknowledged pair carries the same debit as the account's
    /// public closing debit but a different send or receipt body than the
    /// row's terminal outgoing pair. The challenger opens the payer's row
    /// against the change root. Signature bytes are not compared: only
    /// differing bodies contradict.
    ///
    /// Callable by anyone, through the close's challenge deadline.
    ///
    /// # Auth
    /// None.
    pub fn challenge_debit_body(env: &Env, epoch: u64, pair: Pair, row: Row, position: u32, proof: Vec<BytesN<32>>) -> Result<(), Error> {
        let slot = Self::require_challengeable(env, epoch)?;
        Self::verify_pair(env, epoch, &pair)?;
        Self::verify_row_opening(env, &slot.header, position, &row, &proof)?;
        if row.closing.key != pair.send.from {
            return Err(Error::KeyMismatch);
        }
        if pair.send.debit != row.closing.debit {
            return Err(Error::NoContradiction);
        }
        if let Out::Terminal(out) = &row.out {
            if out.send == pair.send && out.receipt == pair.receipt {
                return Err(Error::NoContradiction);
            }
        }
        Self::invalidate(env, epoch, 1);
        Ok(())
    }

    /// Challenge 2a — higher receive-shard tip.
    ///
    /// The challenger authenticates the public tip for one shard — opening
    /// the recipient's row against the change root and the shard's tip
    /// against the row's credit root — and presents a matching retained pair
    /// whose receipt strictly exceeds it in credit or count.
    ///
    /// Callable by anyone, through the close's challenge deadline.
    ///
    /// # Auth
    /// None.
    pub fn challenge_tip(env: &Env, epoch: u64, pair: Pair, row: Row, position: u32, row_proof: Vec<BytesN<32>>, shard_count: u32, tip: ShardTip, tip_proof: Vec<BytesN<32>>) -> Result<(), Error> {
        let slot = Self::require_challengeable(env, epoch)?;
        Self::verify_pair(env, epoch, &pair)?;
        Self::verify_row_opening(env, &slot.header, position, &row, &row_proof)?;
        if row.closing.key != pair.receipt.recipient {
            return Err(Error::KeyMismatch);
        }
        if !merkle::verify_counted(env, &row.credit_root, shard_count, pair.receipt.shard, &tip.digest(env), &tip_proof) {
            return Err(Error::InvalidOpening);
        }
        if pair.receipt.credit <= tip.credit && pair.receipt.count <= tip.count {
            return Err(Error::NoContradiction);
        }
        Self::invalidate(env, epoch, 2);
        Ok(())
    }

    /// Challenge 2b — higher receive-shard tip, absent shard.
    ///
    /// Like `challenge_tip`, but the retained receipt names a shard index at
    /// or beyond the shard count bound in the row's credit root: the
    /// authenticated public tip is `(0, 0)`, which any valid receipt
    /// strictly exceeds. The challenger supplies the credit root's preimage
    /// (shard count and subroot) to authenticate the count.
    ///
    /// Callable by anyone, through the close's challenge deadline.
    ///
    /// # Auth
    /// None.
    pub fn challenge_tip_absent(env: &Env, epoch: u64, pair: Pair, row: Row, position: u32, row_proof: Vec<BytesN<32>>, shard_count: u32, credit_subroot: BytesN<32>) -> Result<(), Error> {
        let slot = Self::require_challengeable(env, epoch)?;
        Self::verify_pair(env, epoch, &pair)?;
        Self::verify_row_opening(env, &slot.header, position, &row, &row_proof)?;
        if row.closing.key != pair.receipt.recipient {
            return Err(Error::KeyMismatch);
        }
        if row.credit_root != merkle::counted(env, shard_count, &credit_subroot) {
            return Err(Error::InvalidOpening);
        }
        if pair.receipt.shard < shard_count {
            return Err(Error::NoContradiction);
        }
        Self::invalidate(env, epoch, 2);
        Ok(())
    }

    /// Challenge 2c — higher receive-shard tip, rowless recipient.
    ///
    /// Like `challenge_tip`, but the close carries no row for the recipient
    /// at all: every shard's authenticated public tip is `(0, 0)`, which any
    /// valid receipt strictly exceeds. The challenger binds the recipient's
    /// key to its registry index by opening its leaf against the closing
    /// state root, then proves row absence for that index (rows are strictly
    /// sorted and count-bound).
    ///
    /// Callable by anyone, through the close's challenge deadline.
    ///
    /// # Auth
    /// None.
    pub fn challenge_tip_no_row(env: &Env, epoch: u64, pair: Pair, account: u32, leaf: AccountLeaf, state_proof: Vec<BytesN<32>>, absence: RowAbsence) -> Result<(), Error> {
        let slot = Self::require_challengeable(env, epoch)?;
        Self::verify_pair(env, epoch, &pair)?;
        Self::verify_state_opening(env, &slot.header.state_root_after, account, &leaf, &state_proof)?;
        if leaf.key != pair.receipt.recipient {
            return Err(Error::KeyMismatch);
        }
        Self::verify_row_absence(env, &slot.header, account, &absence)?;
        Self::invalidate(env, epoch, 2);
        Ok(())
    }

    /// Challenge 3 — inconsistent receipt range.
    ///
    /// For a lower and an upper matching pair in one epoch, recipient, and
    /// shard, adjacent receipts must increase the shard's credit by exactly
    /// the upper payment, and an index gap must leave at least one base unit
    /// for each omitted positive payment. A violation contradicts the
    /// operator regardless of what the close contains.
    ///
    /// Callable by anyone, through the close's challenge deadline.
    ///
    /// # Auth
    /// None.
    pub fn challenge_range(env: &Env, epoch: u64, lower: Pair, upper: Pair) -> Result<(), Error> {
        Self::require_challengeable(env, epoch)?;
        Self::verify_pair(env, epoch, &lower)?;
        Self::verify_pair(env, epoch, &upper)?;
        if lower.receipt.recipient != upper.receipt.recipient || lower.receipt.shard != upper.receipt.shard {
            return Err(Error::PairMismatch);
        }
        if upper.receipt.count <= lower.receipt.count {
            return Err(Error::NoContradiction);
        }
        let gap = upper.receipt.count - lower.receipt.count;
        let diff = upper.receipt.credit - lower.receipt.credit;
        let consistent = if gap == 1 {
            diff == upper.receipt.amount
        } else {
            // Each of the `gap - 1` omitted payments moved the credit by at
            // least one base unit.
            match upper.receipt.amount.checked_add((gap - 1) as i128) {
                Some(minimum) => diff >= minimum,
                None => false,
            }
        };
        if consistent {
            return Err(Error::NoContradiction);
        }
        Self::invalidate(env, epoch, 3);
        Ok(())
    }

    /// Challenge 4 — receipt fork.
    ///
    /// Two distinct receipt bodies signed by the operator in one epoch
    /// either reuse one receipt index within a shard or acknowledge the same
    /// payer transaction differently. Different signature bytes over one
    /// identical receipt body are not a fork.
    ///
    /// Callable by anyone, through the close's challenge deadline.
    ///
    /// # Auth
    /// None.
    pub fn challenge_fork(env: &Env, epoch: u64, first: Receipt, first_sig: BytesN<64>, second: Receipt, second_sig: BytesN<64>) -> Result<(), Error> {
        Self::require_challengeable(env, epoch)?;
        Self::verify_receipt(env, epoch, &first, &first_sig)?;
        Self::verify_receipt(env, epoch, &second, &second_sig)?;
        if first == second {
            return Err(Error::NoContradiction);
        }
        let reused_index = first.recipient == second.recipient && first.shard == second.shard && first.count == second.count;
        let reacknowledged = first.txid == second.txid;
        if !reused_index && !reacknowledged {
            return Err(Error::NoContradiction);
        }
        Self::invalidate(env, epoch, 4);
        Ok(())
    }

    /// Pay a queued exit no finalized close consumed, during terminal
    /// unwind.
    ///
    /// Requires the system frozen with no pending closes remaining. The
    /// caller opens the account's leaf against the last finalized root; the
    /// payment is the exit's amount (or, for a full close, the account's
    /// entire remaining balance), capped by the balance not yet unwound for
    /// the key, and goes to the exit's signed destination.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn unwind_exit(env: &Env, sequence: u32, account: u32, leaf: AccountLeaf, proof: Vec<BytesN<32>>) -> Result<(), Error> {
        Self::require_unwindable(env)?;
        let mut record: ExitRecord = env.storage().persistent().get(&DataKey::Exit(sequence)).ok_or(Error::NoSuchRecord)?;
        if record.paid {
            return Err(Error::AlreadyPaid);
        }
        if sequence < Self::released_boundary(env) {
            // Consumed by a finalized close: release pays it instead.
            return Err(Error::Releasable);
        }
        Self::verify_state_opening(env, &Self::finalized_root(env), account, &leaf, &proof)?;
        if leaf.key != record.key {
            return Err(Error::KeyMismatch);
        }

        let unwound = Self::unwound(env, leaf.key.clone());
        let available = leaf.balance - unwound;
        let amount = if record.full_close { available } else { record.amount.min(available) };
        record.paid = true;
        env.storage().persistent().set(&DataKey::Exit(sequence), &record);
        env.storage().persistent().set(&DataKey::Unwound(leaf.key.clone()), &(unwound + amount));
        Self::pending_exit_sub(env, &record.key, record.amount);

        if amount > 0 {
            Self::token_client(env).transfer(&env.current_contract_address(), &record.destination, &amount);
        }
        env.events().publish_event(&event::UnwindExit {
            sequence,
            destination: record.destination,
            amount,
        });
        Ok(())
    }

    /// Claim an account's remaining balance during terminal unwind.
    ///
    /// Requires the system frozen with no pending closes remaining. The
    /// caller opens the account's leaf against the last finalized root and
    /// presents the account key's signature over a claim payload naming the
    /// destination; see `prepare_claim`. Pays the leaf's balance minus
    /// whatever was already unwound for the key.
    ///
    /// Callable by anyone holding the signed payload.
    ///
    /// # Auth
    /// - Claim signature serves as account key authorization.
    pub fn unwind_claim(env: &Env, account: u32, leaf: AccountLeaf, proof: Vec<BytesN<32>>, destination: Address, sig: BytesN<64>) -> Result<(), Error> {
        Self::require_unwindable(env)?;
        let root = Self::finalized_root(env);
        Self::verify_state_opening(env, &root, account, &leaf, &proof)?;
        if leaf.is_empty(env) {
            return Err(Error::KeyMismatch);
        }
        let body = Claim::new(env.current_contract_address(), destination.clone(), root);
        env.crypto().ed25519_verify(&leaf.key, &body.bytes(), &sig);

        let unwound = Self::unwound(env, leaf.key.clone());
        let amount = leaf.balance - unwound;
        env.storage().persistent().set(&DataKey::Unwound(leaf.key), &leaf.balance);

        if amount > 0 {
            Self::token_client(env).transfer(&env.current_contract_address(), &destination, &amount);
        }
        env.events().publish_event(&event::UnwindClaim { account, destination, amount });
        Ok(())
    }

    /// Refund a deposit no finalized close consumed, during terminal unwind.
    ///
    /// Requires the system frozen with no pending closes remaining. The
    /// refund goes to the recorded depositor.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn unwind_deposit(env: &Env, sequence: u32) -> Result<(), Error> {
        Self::require_unwindable(env)?;
        let mut record: DepositRecord = env.storage().persistent().get(&DataKey::Deposit(sequence)).ok_or(Error::NoSuchRecord)?;
        if record.refunded {
            return Err(Error::AlreadyPaid);
        }
        if sequence < Self::consumed_deposit_boundary(env) {
            // Consumed by a finalized close: the deposit is in the
            // registry's liability and is claimable via unwind_claim.
            return Err(Error::NotReleasable);
        }
        record.refunded = true;
        env.storage().persistent().set(&DataKey::Deposit(sequence), &record);

        Self::token_client(env).transfer(&env.current_contract_address(), &record.depositor, &record.amount);

        env.events().publish_event(&event::UnwindDeposit {
            sequence,
            depositor: record.depositor,
            amount: record.amount,
        });
        Ok(())
    }

    /// Returns the XDR serialized send payload for the given payment.
    ///
    /// The payer signs these bytes with the ed25519 key bound in its account
    /// leaf. `debit` is the payer's cumulative debit after this payment.
    ///
    /// Payloads are typically prepared off-chain. This function is provided
    /// as a convenience.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_send(env: &Env, from: BytesN<32>, to: BytesN<32>, amount: i128, debit: i128, epoch: u64) -> Bytes {
        Send::new(env.current_contract_address(), from, to, amount, debit, epoch).bytes()
    }

    /// Returns the XDR serialized receipt payload for the given shard
    /// advance.
    ///
    /// The operator signs these bytes with the key `operator_key`. `txid` is
    /// the SHA-256 of the acknowledged send payload; `credit` and `count`
    /// are the shard's cumulative credit and receipt count after the
    /// payment.
    ///
    /// Payloads are typically prepared off-chain. This function is provided
    /// as a convenience.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_receipt(env: &Env, recipient: BytesN<32>, shard: u32, amount: i128, txid: BytesN<32>, credit: i128, count: u64, epoch: u64) -> Bytes {
        Receipt::new(env.current_contract_address(), recipient, shard, amount, txid, credit, count, epoch).bytes()
    }

    /// Returns the XDR serialized exit payload for the given withdrawal,
    /// bound to the current finalized state root.
    ///
    /// The account signs these bytes with the ed25519 key bound in its
    /// account leaf, and the signature is passed to `exit`.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_exit(env: &Env, destination: Address, amount: i128, full_close: bool, deadline: u32) -> Bytes {
        Exit::new(env.current_contract_address(), destination, amount, full_close, deadline, Self::finalized_root(env)).bytes()
    }

    /// Returns the XDR serialized claim payload for the given destination,
    /// bound to the current finalized state root.
    ///
    /// The account signs these bytes with the ed25519 key bound in its
    /// account leaf, and the signature is passed to `unwind_claim`.
    ///
    /// Callable by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_claim(env: &Env, destination: Address) -> Bytes {
        Claim::new(env.current_contract_address(), destination, Self::finalized_root(env)).bytes()
    }

    /// Returns the token address.
    pub fn token(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Token).unwrap()
    }

    /// Returns the operator's receipt signing key.
    pub fn operator_key(env: &Env) -> BytesN<32> {
        env.storage().instance().get(&DataKey::OperatorKey).unwrap()
    }

    /// Returns the validator committee's keys, by index.
    pub fn validators(env: &Env) -> Vec<BytesN<32>> {
        env.storage().instance().get(&DataKey::Validators).unwrap()
    }

    /// Returns the certificate quorum size.
    pub fn quorum(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::Quorum).unwrap()
    }

    /// Returns the registry tree depth. Capacity is `2^depth` accounts.
    pub fn registry_depth(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::RegistryDepth).unwrap()
    }

    /// Returns the challenge window in ledgers.
    pub fn challenge_window(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::ChallengeWindow).unwrap()
    }

    /// Returns the minimum number of ledgers between queueing an exit and
    /// its deadline.
    pub fn min_exit_delay(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::MinExitDelay).unwrap()
    }

    /// Returns whether new work is frozen.
    pub fn frozen(env: &Env) -> bool {
        env.storage().instance().get(&DataKey::Frozen).unwrap_or(false)
    }

    /// Returns the next epoch a close can be submitted for. Epochs
    /// `[finalized_epoch, next_epoch)` are pending.
    pub fn next_epoch(env: &Env) -> u64 {
        env.storage().instance().get(&DataKey::NextEpoch).unwrap_or(0)
    }

    /// Returns the number of finalized closes.
    pub fn finalized_epoch(env: &Env) -> u64 {
        env.storage().instance().get(&DataKey::FinalizedEpoch).unwrap_or(0)
    }

    /// Returns the last finalized state root: the genesis root before any
    /// close finalizes. Terminal unwind resolves against this root.
    pub fn finalized_root(env: &Env) -> BytesN<32> {
        let finalized = Self::finalized_epoch(env);
        if finalized == 0 {
            env.storage().instance().get(&DataKey::GenesisRoot).unwrap()
        } else {
            Self::slot(env, finalized - 1).unwrap().header.state_root_after
        }
    }

    /// Returns the total balance the registry owes its accounts at the last
    /// finalized root.
    pub fn finalized_liability(env: &Env) -> i128 {
        let finalized = Self::finalized_epoch(env);
        if finalized == 0 {
            0
        } else {
            Self::slot(env, finalized - 1).unwrap().liability_after
        }
    }

    /// Returns the token balance held by the contract.
    pub fn custody(env: &Env) -> i128 {
        Self::token_client(env).balance(&env.current_contract_address())
    }

    /// Returns a submitted close by epoch: pending or finalized. Invalidated
    /// closes are removed.
    pub fn slot(env: &Env, epoch: u64) -> Option<Slot> {
        env.storage().persistent().get(&DataKey::Slot(epoch))
    }

    /// Returns the number of deposit records.
    pub fn deposit_count(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::DepositCount).unwrap_or(0)
    }

    /// Returns a deposit record by sequence number.
    pub fn deposit_record(env: &Env, sequence: u32) -> Option<DepositRecord> {
        env.storage().persistent().get(&DataKey::Deposit(sequence))
    }

    /// Returns the number of exit records.
    pub fn exit_count(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::ExitCount).unwrap_or(0)
    }

    /// Returns an exit record by sequence number.
    pub fn exit_record(env: &Env, sequence: u32) -> Option<ExitRecord> {
        env.storage().persistent().get(&DataKey::Exit(sequence))
    }

    /// Returns the total queued, unpaid exit amount for a key.
    pub fn pending_exits(env: &Env, key: BytesN<32>) -> i128 {
        env.storage().persistent().get(&DataKey::PendingExit(key)).unwrap_or(0)
    }

    /// Returns the total already paid to a key during terminal unwind.
    pub fn unwound(env: &Env, key: BytesN<32>) -> i128 {
        env.storage().persistent().get(&DataKey::Unwound(key)).unwrap_or(0)
    }
}

impl Contract {
    fn token_client(env: &Env) -> token::Client<'_> {
        token::Client::new(env, &Self::token(env))
    }

    /// The state root the next close must open from.
    fn head_root(env: &Env) -> BytesN<32> {
        let next = Self::next_epoch(env);
        if next == 0 {
            env.storage().instance().get(&DataKey::GenesisRoot).unwrap()
        } else {
            Self::slot(env, next - 1).unwrap().header.state_root_after
        }
    }

    /// The liability the next close opens from.
    fn head_liability(env: &Env) -> i128 {
        let next = Self::next_epoch(env);
        if next == 0 {
            0
        } else {
            Self::slot(env, next - 1).unwrap().liability_after
        }
    }

    /// The boundary record pointers the next close consumes from.
    fn boundary(env: &Env) -> (u32, u32) {
        let next = Self::next_epoch(env);
        if next == 0 {
            (0, 0)
        } else {
            let header = Self::slot(env, next - 1).unwrap().header;
            (header.deposits_to, header.withdrawals_to)
        }
    }

    /// Exit records below this sequence number are consumed by finalized
    /// closes and releasable.
    fn released_boundary(env: &Env) -> u32 {
        let finalized = Self::finalized_epoch(env);
        if finalized == 0 {
            0
        } else {
            Self::slot(env, finalized - 1).unwrap().header.withdrawals_to
        }
    }

    /// Deposit records below this sequence number are consumed by finalized
    /// closes and included in the registry's liability.
    fn consumed_deposit_boundary(env: &Env) -> u32 {
        let finalized = Self::finalized_epoch(env);
        if finalized == 0 {
            0
        } else {
            Self::slot(env, finalized - 1).unwrap().header.deposits_to
        }
    }

    /// The running deposit total through `count` records.
    fn deposit_cumulative(env: &Env, count: u32) -> i128 {
        if count == 0 {
            0
        } else {
            let record: DepositRecord = env.storage().persistent().get(&DataKey::Deposit(count - 1)).unwrap();
            record.cumulative
        }
    }

    /// The running exit total through `count` records.
    fn exit_cumulative(env: &Env, count: u32) -> i128 {
        if count == 0 {
            0
        } else {
            let record: ExitRecord = env.storage().persistent().get(&DataKey::Exit(count - 1)).unwrap();
            record.cumulative
        }
    }

    fn pending_exit_sub(env: &Env, key: &BytesN<32>, amount: i128) {
        let pending = Self::pending_exits(env, key.clone());
        env.storage().persistent().set(&DataKey::PendingExit(key.clone()), &(pending - amount));
    }

    /// The slot for a pending epoch whose challenge window is still open.
    fn require_challengeable(env: &Env, epoch: u64) -> Result<Slot, Error> {
        if epoch < Self::finalized_epoch(env) || epoch >= Self::next_epoch(env) {
            return Err(Error::NoSuchSlot);
        }
        let slot = Self::slot(env, epoch).unwrap();
        if env.ledger().sequence() > slot.deadline {
            return Err(Error::ChallengeWindowClosed);
        }
        Ok(slot)
    }

    /// Requires the system frozen with every pending close resolved.
    fn require_unwindable(env: &Env) -> Result<(), Error> {
        if !Self::frozen(env) {
            return Err(Error::NotFrozen);
        }
        if Self::finalized_epoch(env) != Self::next_epoch(env) {
            return Err(Error::PendingSlotsRemain);
        }
        Ok(())
    }

    /// Removes the challenged close and every pending descendant from the
    /// queue.
    fn invalidate(env: &Env, epoch: u64, kind: u32) {
        let next = Self::next_epoch(env);
        for i in epoch..next {
            env.storage().persistent().remove(&DataKey::Slot(i));
        }
        env.storage().instance().set(&DataKey::NextEpoch, &epoch);
        env.events().publish_event(&event::Invalidate { epoch, kind });
    }

    /// Verifies an account leaf opening against a state root.
    fn verify_state_opening(env: &Env, root: &BytesN<32>, account: u32, leaf: &AccountLeaf, proof: &Vec<BytesN<32>>) -> Result<(), Error> {
        if merkle::verify_fixed(env, root, Self::registry_depth(env), account, &leaf.digest(env), proof) {
            Ok(())
        } else {
            Err(Error::InvalidOpening)
        }
    }

    /// Verifies a row opening against a close's change root.
    fn verify_row_opening(env: &Env, header: &Header, position: u32, row: &Row, proof: &Vec<BytesN<32>>) -> Result<(), Error> {
        if merkle::verify_counted(env, &header.change_root, header.rows, position, &row.digest(env), proof) {
            Ok(())
        } else {
            Err(Error::InvalidOpening)
        }
    }

    /// Verifies that a close contains no row for the given account index.
    fn verify_row_absence(env: &Env, header: &Header, account: u32, absence: &RowAbsence) -> Result<(), Error> {
        match absence {
            RowAbsence::Empty => {
                if header.rows != 0 {
                    return Err(Error::InvalidAbsence);
                }
            }
            RowAbsence::Before(row, proof) => {
                Self::verify_row_opening(env, header, 0, row, proof)?;
                if row.account <= account {
                    return Err(Error::InvalidAbsence);
                }
            }
            RowAbsence::After(row, proof) => {
                if header.rows == 0 {
                    return Err(Error::InvalidAbsence);
                }
                Self::verify_row_opening(env, header, header.rows - 1, row, proof)?;
                if row.account >= account {
                    return Err(Error::InvalidAbsence);
                }
            }
            RowAbsence::Between(position, left, left_proof, right, right_proof) => {
                Self::verify_row_opening(env, header, *position, left, left_proof)?;
                Self::verify_row_opening(env, header, position.checked_add(1).ok_or(Error::InvalidAbsence)?, right, right_proof)?;
                if left.account >= account || right.account <= account {
                    return Err(Error::InvalidAbsence);
                }
            }
        }
        Ok(())
    }

    /// Verifies a matching pair for an epoch: context, linkage between the
    /// send and the receipt, the payer's signature, and the operator's
    /// signature.
    fn verify_pair(env: &Env, epoch: u64, pair: &Pair) -> Result<(), Error> {
        let send = &pair.send;
        if send.domain != symbol_short!("clrsend") || send.network != env.ledger().network_id() || send.contract != env.current_contract_address() || send.epoch != epoch {
            return Err(Error::PairMismatch);
        }
        if send.amount <= 0 || send.debit < send.amount {
            return Err(Error::PairMismatch);
        }
        let receipt = &pair.receipt;
        if receipt.amount != send.amount || receipt.recipient != send.to || receipt.txid != send.txid() {
            return Err(Error::PairMismatch);
        }
        env.crypto().ed25519_verify(&send.from, &send.bytes(), &pair.send_sig);
        Self::verify_receipt(env, epoch, receipt, &pair.receipt_sig)
    }

    /// Verifies a receipt's context and the operator's signature over it.
    fn verify_receipt(env: &Env, epoch: u64, receipt: &Receipt, sig: &BytesN<64>) -> Result<(), Error> {
        if receipt.domain != symbol_short!("clrrcpt") || receipt.network != env.ledger().network_id() || receipt.contract != env.current_contract_address() || receipt.epoch != epoch {
            return Err(Error::PairMismatch);
        }
        if receipt.amount <= 0 || receipt.credit < receipt.amount || receipt.count == 0 {
            return Err(Error::PairMismatch);
        }
        env.crypto().ed25519_verify(&Self::operator_key(env), &receipt.bytes(), sig);
        Ok(())
    }
}

mod test;
mod testutils;
