use soroban_sdk::{contractevent, Address, BytesN};

/// Emitted when the clearing system is deployed via the constructor.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Setup {
    /// The SEP-41 token held in custody.
    pub token: Address,
    /// The ed25519 key the operator signs receipts with.
    pub operator_key: BytesN<32>,
    /// The number of validators in the committee.
    pub validators: u32,
    /// The number of validator signatures a close certificate requires.
    pub quorum: u32,
    /// The depth of the account registry tree (capacity `2^depth`).
    pub registry_depth: u32,
    /// The number of ledgers a submitted close remains challengeable.
    pub challenge_window: u32,
    /// The minimum number of ledgers between queueing an exit and its
    /// deadline.
    pub min_exit_delay: u32,
}

/// Emitted when tokens are deposited for an account key.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Deposit {
    /// The sequence number of the deposit record.
    pub sequence: u32,
    /// The address the tokens were transferred from.
    pub depositor: Address,
    /// The account key the deposit is for.
    pub key: BytesN<32>,
    /// The amount deposited.
    pub amount: i128,
}

/// Emitted when a signed withdrawal is queued.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitQueued {
    /// The sequence number of the exit record.
    pub sequence: u32,
    /// The account key the exit debits.
    pub key: BytesN<32>,
    /// The destination the exit pays.
    pub destination: Address,
    /// The amount to withdraw.
    pub amount: i128,
    /// Whether the account asks to be closed entirely.
    pub full_close: bool,
    /// The ledger sequence number after which an unreleased exit freezes the
    /// system.
    pub deadline: u32,
}

/// Emitted when a close is submitted and enters the pending queue.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Submit {
    /// The epoch the close settles.
    pub epoch: u64,
    /// The state root the close transitions to.
    pub state_root_after: BytesN<32>,
    /// The ledger sequence number through which the close can be challenged
    /// (inclusive).
    pub deadline: u32,
}

/// Emitted when a pending close survives its challenge window and finalizes.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finalize {
    /// The epoch that finalized.
    pub epoch: u64,
    /// The finalized state root.
    pub state_root: BytesN<32>,
    /// The total balance the registry owes its accounts after the close.
    pub liability: i128,
}

/// Emitted when a challenge proves a contradiction and invalidates a pending
/// close and all of its pending descendants.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invalidate {
    /// The first invalidated epoch. Every pending epoch at or after it is
    /// removed from the queue.
    pub epoch: u64,
    /// The challenge kind that proved the contradiction: 1 payer debit
    /// contradiction, 2 higher receive-shard tip, 3 inconsistent receipt
    /// range, 4 receipt fork.
    pub kind: u32,
}

/// Emitted when a finalized withdrawal is released to its destination.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Release {
    /// The sequence number of the exit record.
    pub sequence: u32,
    /// The destination paid.
    pub destination: Address,
    /// The amount paid.
    pub amount: i128,
}

/// Emitted when an exit deadline passes unreleased and the system freezes.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Freeze {
    /// The exit record whose deadline was breached.
    pub sequence: u32,
}

/// Emitted when a queued exit is paid during terminal unwind.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnwindExit {
    /// The sequence number of the exit record.
    pub sequence: u32,
    /// The destination paid.
    pub destination: Address,
    /// The amount paid.
    pub amount: i128,
}

/// Emitted when an account claims its remaining balance during terminal
/// unwind.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnwindClaim {
    /// The registry index of the account.
    pub account: u32,
    /// The destination paid.
    pub destination: Address,
    /// The amount paid.
    pub amount: i128,
}

/// Emitted when an unconsumed deposit is refunded during terminal unwind.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnwindDeposit {
    /// The sequence number of the deposit record.
    pub sequence: u32,
    /// The depositor refunded.
    pub depositor: Address,
    /// The amount refunded.
    pub amount: i128,
}
