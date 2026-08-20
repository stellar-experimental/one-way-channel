//! SHA-256 Merkle tree primitives shared by the clearing contract and its
//! off-chain builders.
//!
//! Three domain-separated hash constructions prevent collisions between
//! kinds of nodes:
//!
//! - Leaf digest: `sha256(0x00 || bytes)`.
//! - Inner node: `sha256(0x01 || left || right)`.
//! - Counted root: `sha256(0x02 || be_bytes(count) || subroot)`, binding the
//!   exact number of leaves of a dynamically sized tree.
//!
//! Dynamically sized trees are padded to the next power of two with the
//! all-zero digest. The all-zero digest is also the digest of an empty
//! position in the fixed-capacity account registry tree, so an unregistered
//! account slot and Merkle padding hash identically to "nothing here".

use soroban_sdk::{Bytes, BytesN, Env, Vec};

/// The digest of an empty tree position: 32 zero bytes.
pub fn empty(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[0u8; 32])
}

/// SHA-256 of arbitrary bytes as a `BytesN<32>`.
pub fn sha256(env: &Env, bytes: &Bytes) -> BytesN<32> {
    env.crypto().sha256(bytes).to_bytes()
}

/// Digest of a leaf: `sha256(0x00 || bytes)`.
pub fn leaf(env: &Env, bytes: &Bytes) -> BytesN<32> {
    let mut buf = Bytes::from_array(env, &[0x00]);
    buf.append(bytes);
    sha256(env, &buf)
}

/// Digest of an inner node: `sha256(0x01 || left || right)`.
pub fn node(env: &Env, left: &BytesN<32>, right: &BytesN<32>) -> BytesN<32> {
    let mut buf = Bytes::from_array(env, &[0x01]);
    buf.append(&Bytes::from_array(env, &left.to_array()));
    buf.append(&Bytes::from_array(env, &right.to_array()));
    sha256(env, &buf)
}

/// Root of a dynamically sized tree binding the exact leaf count:
/// `sha256(0x02 || be_bytes(count) || subroot)`.
pub fn counted(env: &Env, count: u32, subroot: &BytesN<32>) -> BytesN<32> {
    let mut buf = Bytes::from_array(env, &[0x02]);
    buf.append(&Bytes::from_array(env, &count.to_be_bytes()));
    buf.append(&Bytes::from_array(env, &subroot.to_array()));
    sha256(env, &buf)
}

/// The depth of the padded subtree holding `count` leaves: `ceil(log2(count))`,
/// with zero for one or fewer leaves.
pub fn depth_for(count: u32) -> u32 {
    if count <= 1 {
        0
    } else {
        32 - (count - 1).leading_zeros()
    }
}

/// Folds a Merkle authentication path from a leaf digest up to the subtree
/// root it implies. The path lists sibling digests from bottom to top; the
/// index selects left/right at each level.
pub fn fold(env: &Env, index: u32, leaf_digest: &BytesN<32>, path: &Vec<BytesN<32>>) -> BytesN<32> {
    let mut current = leaf_digest.clone();
    for (level, sibling) in path.iter().enumerate() {
        if (index >> level) & 1 == 1 {
            current = node(env, &sibling, &current);
        } else {
            current = node(env, &current, &sibling);
        }
    }
    current
}

/// Verifies a leaf opening against a counted root: the index must be within
/// the bound count, the path must have the exact depth implied by the count,
/// and the folded subroot must recompute the counted root.
pub fn verify_counted(env: &Env, root: &BytesN<32>, count: u32, index: u32, leaf_digest: &BytesN<32>, path: &Vec<BytesN<32>>) -> bool {
    if index >= count || path.len() != depth_for(count) {
        return false;
    }
    let subroot = fold(env, index, leaf_digest, path);
    counted(env, count, &subroot) == *root
}

/// Verifies a leaf opening against the root of a fixed-capacity tree of the
/// given depth (the account registry).
pub fn verify_fixed(env: &Env, root: &BytesN<32>, depth: u32, index: u32, leaf_digest: &BytesN<32>, path: &Vec<BytesN<32>>) -> bool {
    if path.len() != depth || (depth < 32 && u64::from(index) >= (1u64 << depth)) {
        return false;
    }
    fold(env, index, leaf_digest, path) == *root
}

/// The root of a fixed-capacity tree of the given depth with every position
/// empty. This is the genesis state root of a registry with no accounts.
pub fn empty_root(env: &Env, depth: u32) -> BytesN<32> {
    let mut current = empty(env);
    for _ in 0..depth {
        current = node(env, &current, &current);
    }
    current
}
