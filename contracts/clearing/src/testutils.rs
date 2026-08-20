#![cfg(test)]
//! Off-chain machinery for exercising the clearing contract in tests: the
//! operator that serves payments and builds closes, the validator committee
//! that exhaustively checks a close's public corpus before signing, and the
//! Merkle builders (state tree, change tree, credit trees, and the paired
//! sparse witness) the contract only ever verifies.

extern crate std;

use std::vec;
use std::vec::Vec as StdVec;

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, Bytes, BytesN, Env, Symbol, Vec,
};

use crate::{merkle, AccountLeaf, Claim, Contract, ContractClient, Exit, Header, Out, Pair, Prefix, Receipt, Row, Send, ShardTip, Signature};

/// Registry depth used by tests: capacity 8 accounts.
pub const DEPTH: u32 = 3;
/// Challenge window in ledgers used by tests.
pub const WINDOW: u32 = 100;
/// Minimum exit delay in ledgers used by tests.
pub const EXIT_DELAY: u32 = 50;
/// Committee size used by tests.
pub const VALIDATORS: usize = 4;
/// Certificate quorum used by tests.
pub const QUORUM: u32 = 3;

pub fn sign(env: &Env, sk: &SigningKey, payload: &Bytes) -> BytesN<64> {
    let raw: StdVec<u8> = payload.iter().collect();
    BytesN::from_array(env, &sk.sign(&raw).to_bytes())
}

pub fn key_of(env: &Env, sk: &SigningKey) -> BytesN<32> {
    BytesN::from_array(env, &sk.verifying_key().to_bytes())
}

/// A deployed clearing contract with its token, operator key, and validator
/// committee.
pub struct Harness {
    pub env: Env,
    pub id: Address,
    pub token: Address,
    pub operator: SigningKey,
    pub validators: StdVec<SigningKey>,
}

impl Harness {
    pub fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(admin).address();

        let operator = SigningKey::from_bytes(&[99u8; 32]);
        let validators: StdVec<SigningKey> = (0..VALIDATORS).map(|i| SigningKey::from_bytes(&[200 + i as u8; 32])).collect();
        let mut committee: Vec<BytesN<32>> = Vec::new(&env);
        for v in &validators {
            committee.push_back(key_of(&env, v));
        }

        let id = env.register(Contract, (token.clone(), key_of(&env, &operator), committee, QUORUM, DEPTH, WINDOW, EXIT_DELAY));
        Harness { env, id, token, operator, validators }
    }

    pub fn client(&self) -> ContractClient<'_> {
        ContractClient::new(&self.env, &self.id)
    }

    pub fn token_client(&self) -> TokenClient<'_> {
        TokenClient::new(&self.env, &self.token)
    }

    pub fn mint(&self, to: &Address, amount: i128) {
        StellarAssetClient::new(&self.env, &self.token).mint(to, &amount);
    }

    pub fn pass(&self, ledgers: u32) {
        self.env.ledger().with_mut(|li| li.sequence_number += ledgers);
    }

    pub fn now(&self) -> u32 {
        self.env.ledger().sequence()
    }

    /// A full-committee certificate over a header.
    pub fn cert(&self, header: &Header) -> Vec<Signature> {
        let indices: StdVec<usize> = (0..self.validators.len()).collect();
        self.cert_of(header, &indices)
    }

    /// A certificate signed by the given validator indices.
    pub fn cert_of(&self, header: &Header, indices: &[usize]) -> Vec<Signature> {
        let payload = header.bytes();
        let mut out: Vec<Signature> = Vec::new(&self.env);
        for &i in indices {
            out.push_back(Signature {
                index: i as u32,
                signature: sign(&self.env, &self.validators[i], &payload),
            });
        }
        out
    }

    /// The operator's signature over a receipt body.
    pub fn sign_receipt(&self, receipt: &Receipt) -> BytesN<64> {
        sign(&self.env, &self.operator, &receipt.bytes())
    }
}

// Merkle builders. The contract only verifies openings; building the trees
// is off-chain work.

fn levels(env: &Env, digests: &StdVec<BytesN<32>>) -> StdVec<StdVec<BytesN<32>>> {
    let depth = merkle::depth_for(digests.len() as u32);
    let mut level = digests.clone();
    level.resize(1 << depth, merkle::empty(env));
    let mut out = vec![level];
    while out.last().unwrap().len() > 1 {
        let next: StdVec<BytesN<32>> = out.last().unwrap().chunks(2).map(|p| merkle::node(env, &p[0], &p[1])).collect();
        out.push(next);
    }
    out
}

/// The root of the padded subtree over the given digests.
pub fn subtree_root(env: &Env, digests: &StdVec<BytesN<32>>) -> BytesN<32> {
    levels(env, digests).last().unwrap()[0].clone()
}

/// The authentication path for the digest at `index`.
pub fn subtree_proof(env: &Env, digests: &StdVec<BytesN<32>>, index: usize) -> Vec<BytesN<32>> {
    let mut proof: Vec<BytesN<32>> = Vec::new(env);
    let mut idx = index;
    for level in levels(env, digests) {
        if level.len() == 1 {
            break;
        }
        proof.push_back(level[idx ^ 1].clone());
        idx /= 2;
    }
    proof
}

pub fn state_digests(env: &Env, registry: &StdVec<AccountLeaf>) -> StdVec<BytesN<32>> {
    registry.iter().map(|l| l.digest(env)).collect()
}

/// The state root of a full-capacity registry.
pub fn state_root(env: &Env, registry: &StdVec<AccountLeaf>) -> BytesN<32> {
    subtree_root(env, &state_digests(env, registry))
}

/// The opening of the account leaf at `index` against the registry's state
/// root.
pub fn state_proof(env: &Env, registry: &StdVec<AccountLeaf>, index: usize) -> Vec<BytesN<32>> {
    subtree_proof(env, &state_digests(env, registry), index)
}

pub fn row_digests(env: &Env, rows: &StdVec<Row>) -> StdVec<BytesN<32>> {
    rows.iter().map(|r| r.digest(env)).collect()
}

/// The change root binding the exact row count and every row in order.
pub fn change_root(env: &Env, rows: &StdVec<Row>) -> BytesN<32> {
    merkle::counted(env, rows.len() as u32, &subtree_root(env, &row_digests(env, rows)))
}

/// The opening of the row at `position` against the change root.
pub fn row_proof(env: &Env, rows: &StdVec<Row>, position: usize) -> Vec<BytesN<32>> {
    subtree_proof(env, &row_digests(env, rows), position)
}

pub fn tip_digests(env: &Env, tips: &StdVec<ShardTip>) -> StdVec<BytesN<32>> {
    tips.iter().map(|t| t.digest(env)).collect()
}

/// The credit root binding the exact shard count and every tip in shard
/// order.
pub fn credit_root(env: &Env, tips: &StdVec<ShardTip>) -> BytesN<32> {
    merkle::counted(env, tips.len() as u32, &subtree_root(env, &tip_digests(env, tips)))
}

/// The subroot inside a credit root (its preimage alongside the count).
pub fn credit_subroot(env: &Env, tips: &StdVec<ShardTip>) -> BytesN<32> {
    subtree_root(env, &tip_digests(env, tips))
}

/// The opening of the tip at shard `kappa` against the credit root.
pub fn tip_proof(env: &Env, tips: &StdVec<ShardTip>, kappa: usize) -> Vec<BytesN<32>> {
    subtree_proof(env, &tip_digests(env, tips), kappa)
}

/// The paired sparse witness: every changed account supplies its opening and
/// closing leaf while each maximal untouched subtree contributes one shared
/// frontier digest. One pass reconstructs both state roots from the same
/// material, proving every omitted account unchanged.
#[derive(Clone)]
pub struct Witness {
    pub changed: StdVec<(usize, AccountLeaf, AccountLeaf)>,
    pub frontier: StdVec<BytesN<32>>,
}

pub fn build_witness(env: &Env, opening: &StdVec<AccountLeaf>, closing: &StdVec<AccountLeaf>) -> Witness {
    let changed: StdVec<(usize, AccountLeaf, AccountLeaf)> = (0..opening.len()).filter(|&i| opening[i] != closing[i]).map(|i| (i, opening[i].clone(), closing[i].clone())).collect();
    let mut frontier = StdVec::new();
    collect_frontier(env, opening, &changed, 0, opening.len(), &mut frontier);
    Witness { changed, frontier }
}

fn collect_frontier(env: &Env, opening: &StdVec<AccountLeaf>, changed: &StdVec<(usize, AccountLeaf, AccountLeaf)>, lo: usize, hi: usize, frontier: &mut StdVec<BytesN<32>>) {
    if !changed.iter().any(|(i, _, _)| lo <= *i && *i < hi) {
        let digests = opening[lo..hi].iter().map(|l| l.digest(env)).collect();
        frontier.push(subtree_root(env, &digests));
        return;
    }
    if hi - lo == 1 {
        // A changed leaf: supplied by the witness's changed pairs.
        return;
    }
    let mid = lo + (hi - lo) / 2;
    collect_frontier(env, opening, changed, lo, mid, frontier);
    collect_frontier(env, opening, changed, mid, hi, frontier);
}

/// Recomputes both state roots from the witness in one pass. Returns `None`
/// if the witness is malformed.
pub fn verify_witness(env: &Env, witness: &Witness, capacity: usize) -> Option<(BytesN<32>, BytesN<32>)> {
    let mut cursor = 0usize;
    let roots = witness_range(env, witness, 0, capacity, &mut cursor)?;
    if cursor != witness.frontier.len() {
        return None;
    }
    Some(roots)
}

fn witness_range(env: &Env, witness: &Witness, lo: usize, hi: usize, cursor: &mut usize) -> Option<(BytesN<32>, BytesN<32>)> {
    if !witness.changed.iter().any(|(i, _, _)| lo <= *i && *i < hi) {
        let digest = witness.frontier.get(*cursor)?.clone();
        *cursor += 1;
        return Some((digest.clone(), digest));
    }
    if hi - lo == 1 {
        let (_, x0, x1) = witness.changed.iter().find(|(i, _, _)| *i == lo)?;
        return Some((x0.digest(env), x1.digest(env)));
    }
    let mid = lo + (hi - lo) / 2;
    let (l0, l1) = witness_range(env, witness, lo, mid, cursor)?;
    let (r0, r1) = witness_range(env, witness, mid, hi, cursor)?;
    Some((merkle::node(env, &l0, &r0), merkle::node(env, &l1, &r1)))
}

/// A close's complete public corpus: the header, every row, the shard tip
/// vector behind every credit root, and the paired sparse witness. This is
/// what the validator committee checks and retains, and what challengers
/// build openings from.
#[derive(Clone)]
pub struct CloseBundle {
    pub header: Header,
    pub rows: StdVec<Row>,
    /// Shard tips aligned with `rows`: `shard_tips[i]` are the tips behind
    /// `rows[i].credit_root`.
    pub shard_tips: StdVec<StdVec<ShardTip>>,
    pub witness: Witness,
    pub opening: StdVec<AccountLeaf>,
    pub closing: StdVec<AccountLeaf>,
}

impl CloseBundle {
    pub fn terminal(&self) -> Option<Row> {
        self.rows.last().cloned()
    }

    pub fn terminal_proof(&self, env: &Env) -> Vec<BytesN<32>> {
        if self.rows.is_empty() {
            Vec::new(env)
        } else {
            row_proof(env, &self.rows, self.rows.len() - 1)
        }
    }

    /// Submits this close with a full-committee certificate.
    pub fn submit(&self, h: &Harness) {
        h.client().submit(&self.header, &h.cert(&self.header), &self.terminal(), &self.terminal_proof(&h.env));
    }

    /// The position of the row for `account`, if any.
    pub fn position(&self, account: usize) -> Option<usize> {
        self.rows.iter().position(|r| r.account == account as u32)
    }
}

/// The validator committee's exhaustive pre-queue validation: every row,
/// every prefix, the exact state transition via the paired witness, credit
/// root reconstruction, terminal pair authentication, and the conservation
/// laws. Validators sign the header only when all of it holds. (It cannot
/// establish that the corpus contains every receipt the operator signed and
/// delivered privately — that is what the challenges are for.)
pub fn validate_close(h: &Harness, bundle: &CloseBundle) -> bool {
    let env = &h.env;
    let header = &bundle.header;
    let capacity = 1usize << DEPTH;
    if bundle.opening.len() != capacity || bundle.closing.len() != capacity {
        return false;
    }
    if header.rows != bundle.rows.len() as u32 || bundle.shard_tips.len() != bundle.rows.len() {
        return false;
    }

    // Rows: strictly sorted, prefix-continuous, and exact per-account
    // transitions.
    let mut previous_account: Option<u32> = None;
    let mut running = Prefix {
        credits: 0,
        debits: 0,
        deposits: 0,
        shards: 0,
        withdrawal_records: 0,
        withdrawals: 0,
    };
    for (row, tips) in bundle.rows.iter().zip(bundle.shard_tips.iter()) {
        if previous_account.is_some_and(|p| row.account <= p) {
            return false;
        }
        previous_account = Some(row.account);
        let account = row.account as usize;
        if account >= capacity || bundle.opening[account] != row.opening || bundle.closing[account] != row.closing {
            return false;
        }

        // Key immutability and registration.
        if !row.opening.is_empty(env) && row.opening.key != row.closing.key {
            return false;
        }

        // Checked deltas.
        let d = row.closing.debit - row.opening.debit;
        let c = row.closing.credit - row.opening.credit;
        let j = row.closing.receipts as i128 - row.opening.receipts as i128;
        if d < 0 || c < 0 || j < 0 {
            return false;
        }

        // The credit root binds the exact shard vector, whose totals are
        // the account's credit and receipt deltas.
        if row.credit_root != credit_root(env, tips) {
            return false;
        }
        if tips.iter().map(|t| t.credit).sum::<i128>() != c || tips.iter().map(|t| t.count).sum::<u64>() as i128 != j {
            return false;
        }

        // The terminal outgoing pair is present exactly when the account
        // sent, matches the closing debit, and is fully authenticated.
        match &row.out {
            Out::Terminal(pair) => {
                if d == 0 || pair.send.from != row.closing.key || pair.send.debit != row.closing.debit || pair.send.epoch != header.epoch {
                    return false;
                }
                if pair.receipt.txid != pair.send.txid() || pair.receipt.amount != pair.send.amount || pair.receipt.recipient != pair.send.to {
                    return false;
                }
                let raw_send: StdVec<u8> = pair.send.bytes().iter().collect();
                let raw_receipt: StdVec<u8> = pair.receipt.bytes().iter().collect();
                use ed25519_dalek::{Signature as DalekSignature, Verifier, VerifyingKey};
                let payer = VerifyingKey::from_bytes(&pair.send.from.to_array());
                let payer_ok = payer.is_ok_and(|k| k.verify(&raw_send, &DalekSignature::from_bytes(&pair.send_sig.to_array())).is_ok());
                let operator_ok = h.operator.verifying_key().verify(&raw_receipt, &DalekSignature::from_bytes(&pair.receipt_sig.to_array())).is_ok();
                if !payer_ok || !operator_ok {
                    return false;
                }
            }
            Out::Absent => {
                if d != 0 {
                    return false;
                }
            }
        }

        // Prefix continuity: each prefix extends the preceding prefix
        // exactly, and the per-account boundary flows close the balance
        // equation B1 + d + w = B0 + c + f.
        let f = row.prefix.deposits - running.deposits;
        let w = row.prefix.withdrawals - running.withdrawals;
        if f < 0 || w < 0 {
            return false;
        }
        if row.closing.balance + d + w != row.opening.balance + c + f {
            return false;
        }
        running.credits += c;
        running.debits += d;
        running.deposits = row.prefix.deposits;
        running.shards += tips.len() as u64;
        running.withdrawals = row.prefix.withdrawals;
        running.withdrawal_records = row.prefix.withdrawal_records;
        if running != row.prefix {
            return false;
        }
    }

    // The terminal prefix alone carries the epoch's totals, and payments
    // conserve.
    if running.debits != header.debits
        || running.credits != header.credits
        || running.deposits != header.deposits
        || running.withdrawals != header.withdrawals
        || running.withdrawal_records != header.withdrawal_records
        || running.shards != header.shards
    {
        return false;
    }
    if header.debits != header.credits {
        return false;
    }

    // Every account changes if and only if it has a row.
    for i in 0..capacity {
        let has_row = bundle.rows.iter().any(|r| r.account as usize == i);
        if !has_row && bundle.opening[i] != bundle.closing[i] {
            return false;
        }
    }

    // The paired sparse witness reconstructs both roots from the same
    // material.
    if bundle.witness.changed.len() != bundle.rows.len() {
        return false;
    }
    for ((i, x0, x1), row) in bundle.witness.changed.iter().zip(bundle.rows.iter()) {
        if *i != row.account as usize || *x0 != row.opening || *x1 != row.closing {
            return false;
        }
    }
    match verify_witness(env, &bundle.witness, capacity) {
        Some((root, root_after)) => root == header.state_root && root_after == header.state_root_after,
        None => false,
    }
}

/// The off-chain operator: serves payments against live account state,
/// countersigns receipts, and builds each epoch's close. Cloning an engine
/// forks its world — the fraud tests build a close from a fork that omits
/// accepted payments, which validates cleanly (omission is internally
/// consistent) and falls only to a retained receipt.
#[derive(Clone)]
pub struct Engine {
    pub env: Env,
    pub contract: Address,
    pub epoch: u64,
    pub opening: StdVec<AccountLeaf>,
    pub live: StdVec<AccountLeaf>,
    pub shards: StdVec<StdVec<ShardTip>>,
    pub outs: StdVec<Option<Pair>>,
    pub deposits_in: StdVec<i128>,
    pub withdrawals_out: StdVec<i128>,
    pub withdrawal_records: StdVec<u32>,
    pub deposit_log: StdVec<(BytesN<32>, i128)>,
    pub exit_log: StdVec<(BytesN<32>, i128)>,
    pub deposits_to: u32,
    pub withdrawals_to: u32,
    /// `history[e]` is the registry at `StateRoot_e`.
    pub history: StdVec<StdVec<AccountLeaf>>,
}

impl Engine {
    pub fn new(h: &Harness) -> Self {
        let capacity = 1usize << DEPTH;
        let registry: StdVec<AccountLeaf> = vec![AccountLeaf::empty(&h.env); capacity];
        Engine {
            env: h.env.clone(),
            contract: h.id.clone(),
            epoch: 0,
            opening: registry.clone(),
            live: registry.clone(),
            shards: vec![StdVec::new(); capacity],
            outs: vec![None; capacity],
            deposits_in: vec![0; capacity],
            withdrawals_out: vec![0; capacity],
            withdrawal_records: vec![0; capacity],
            deposit_log: StdVec::new(),
            exit_log: StdVec::new(),
            deposits_to: 0,
            withdrawals_to: 0,
            history: vec![registry],
        }
    }

    pub fn key(&self, account: usize) -> BytesN<32> {
        self.live[account].key.clone()
    }

    fn find(&self, key: &BytesN<32>) -> Option<usize> {
        self.live.iter().position(|l| !l.is_empty(&self.env) && l.key == *key)
    }

    /// Deposits on-chain and records the boundary event.
    pub fn deposit(&mut self, h: &Harness, depositor: &Address, key: &BytesN<32>, amount: i128) -> u32 {
        let sequence = h.client().deposit(depositor, key, &amount);
        self.deposit_log.push((key.clone(), amount));
        sequence
    }

    /// Queues an exit on-chain — building the affordability proof against
    /// the finalized root — and records the boundary event.
    pub fn exit(&mut self, h: &Harness, account: usize, signer: &SigningKey, destination: &Address, amount: i128, full_close: bool, deadline: u32) -> u32 {
        let finalized = h.client().finalized_epoch() as usize;
        let registry = &self.history[finalized];
        let root = state_root(&self.env, registry);
        let body = Exit::new(self.contract.clone(), destination.clone(), amount, full_close, deadline, root);
        let sig = sign(&self.env, signer, &body.bytes());
        let leaf = registry[account].clone();
        let proof = state_proof(&self.env, registry, account);
        let sequence = h.client().exit(destination, &amount, &full_close, &deadline, &sig, &(account as u32), &leaf, &proof);
        self.exit_log.push((leaf.key, amount));
        sequence
    }

    /// Fixes the epoch's chain-sealed boundary: consumes every recorded
    /// deposit and exit, registering new keys at free registry indices.
    /// Deposits and withdrawals are fixed before online payments begin.
    pub fn begin_epoch(&mut self) {
        for i in self.deposits_to as usize..self.deposit_log.len() {
            let (key, amount) = self.deposit_log[i].clone();
            let account = match self.find(&key) {
                Some(a) => a,
                None => {
                    let free = self.live.iter().position(|l| l.is_empty(&self.env)).expect("registry full");
                    self.live[free].key = key;
                    self.live[free].active = true;
                    free
                }
            };
            self.live[account].balance += amount;
            self.deposits_in[account] += amount;
        }
        self.deposits_to = self.deposit_log.len() as u32;

        for i in self.withdrawals_to as usize..self.exit_log.len() {
            let (key, amount) = self.exit_log[i].clone();
            let account = self.find(&key).expect("exit for unknown key");
            self.live[account].balance -= amount;
            self.withdrawals_out[account] += amount;
            self.withdrawal_records[account] += 1;
        }
        self.withdrawals_to = self.exit_log.len() as u32;
    }

    /// Serves one payment: the payer signs the exact next debit, the
    /// operator verifies spendability, atomically commits the debit and the
    /// shard advance, and countersigns the receipt. Returns the matching
    /// pair — the accepted payment, the preconfirmation, and the evidence.
    pub fn pay(&mut self, h: &Harness, from: usize, to: usize, amount: i128, shard: usize, payer: &SigningKey) -> Pair {
        assert!(amount > 0);
        assert!(self.live[from].balance >= amount, "spendability");
        assert_eq!(self.live[from].key, key_of(&self.env, payer), "payer key");

        let send = Send::new(self.contract.clone(), self.key(from), self.key(to), amount, self.live[from].debit + amount, self.epoch);
        let send_sig = sign(&self.env, payer, &send.bytes());

        assert!(shard <= self.shards[to].len(), "shards are dense");
        if shard == self.shards[to].len() {
            self.shards[to].push(ShardTip { count: 0, credit: 0 });
        }
        let tip = self.shards[to][shard].clone();
        let receipt = Receipt::new(self.contract.clone(), self.key(to), shard as u32, amount, send.txid(), tip.credit + amount, tip.count + 1, self.epoch);
        let receipt_sig = h.sign_receipt(&receipt);

        // Atomically commit the debit, shard advance, and receipt.
        self.live[from].balance -= amount;
        self.live[from].debit += amount;
        self.live[to].balance += amount;
        self.live[to].credit += amount;
        self.live[to].receipts += 1;
        self.shards[to][shard] = ShardTip {
            count: tip.count + 1,
            credit: tip.credit + amount,
        };

        let pair = Pair { receipt, receipt_sig, send, send_sig };
        self.outs[from] = Some(pair.clone());
        pair
    }

    /// One row per changed account, strictly sorted, with running prefix
    /// totals.
    fn rows(&self) -> (StdVec<Row>, StdVec<StdVec<ShardTip>>) {
        let mut rows = StdVec::new();
        let mut tips = StdVec::new();
        let mut prefix = Prefix {
            credits: 0,
            debits: 0,
            deposits: 0,
            shards: 0,
            withdrawal_records: 0,
            withdrawals: 0,
        };
        for account in 0..self.live.len() {
            if self.opening[account] == self.live[account] && self.deposits_in[account] == 0 && self.withdrawals_out[account] == 0 {
                continue;
            }
            prefix.credits += self.live[account].credit - self.opening[account].credit;
            prefix.debits += self.live[account].debit - self.opening[account].debit;
            prefix.deposits += self.deposits_in[account];
            prefix.shards += self.shards[account].len() as u64;
            prefix.withdrawal_records += self.withdrawal_records[account];
            prefix.withdrawals += self.withdrawals_out[account];
            rows.push(Row {
                account: account as u32,
                closing: self.live[account].clone(),
                credit_root: credit_root(&self.env, &self.shards[account]),
                opening: self.opening[account].clone(),
                out: match &self.outs[account] {
                    Some(pair) => Out::Terminal(pair.clone()),
                    None => Out::Absent,
                },
                prefix: prefix.clone(),
            });
            tips.push(self.shards[account].clone());
        }
        (rows, tips)
    }

    /// Builds the epoch's close — rows, roots, witness, header — validates
    /// it as the committee would, and rolls the engine into the next epoch.
    pub fn build_close(&mut self, h: &Harness) -> CloseBundle {
        let (rows, shard_tips) = self.rows();
        let totals = rows.last().map(|r| r.prefix.clone()).unwrap_or(Prefix {
            credits: 0,
            debits: 0,
            deposits: 0,
            shards: 0,
            withdrawal_records: 0,
            withdrawals: 0,
        });
        let header = Header {
            change_root: change_root(&self.env, &rows),
            contract: self.contract.clone(),
            credits: totals.credits,
            debits: totals.debits,
            deposits: totals.deposits,
            deposits_to: self.deposits_to,
            domain: Symbol::new(&self.env, "clrhead"),
            epoch: self.epoch,
            network: self.env.ledger().network_id(),
            rows: rows.len() as u32,
            shards: totals.shards,
            state_root: state_root(&self.env, &self.opening),
            state_root_after: state_root(&self.env, &self.live),
            withdrawal_records: totals.withdrawal_records,
            withdrawals: totals.withdrawals,
            withdrawals_to: self.withdrawals_to,
        };
        let bundle = CloseBundle {
            header,
            rows,
            shard_tips,
            witness: build_witness(&self.env, &self.opening, &self.live),
            opening: self.opening.clone(),
            closing: self.live.clone(),
        };
        assert!(validate_close(h, &bundle), "honest closes must validate");

        // Roll over: the preserved head carries forward and the next epoch
        // serves on it while this close is certified and queued.
        self.history.push(self.live.clone());
        self.opening = self.live.clone();
        let capacity = self.live.len();
        self.shards = vec![StdVec::new(); capacity];
        self.outs = vec![None; capacity];
        self.deposits_in = vec![0; capacity];
        self.withdrawals_out = vec![0; capacity];
        self.withdrawal_records = vec![0; capacity];
        self.epoch += 1;
        bundle
    }

    /// A claim signature for terminal unwind, over the finalized root.
    pub fn claim_sig(&self, h: &Harness, signer: &SigningKey, destination: &Address) -> BytesN<64> {
        let finalized = h.client().finalized_epoch() as usize;
        let root = state_root(&self.env, &self.history[finalized]);
        let body = Claim::new(self.contract.clone(), destination.clone(), root);
        sign(&self.env, signer, &body.bytes())
    }
}
