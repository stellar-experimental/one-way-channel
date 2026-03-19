#!/bin/bash
set -euo pipefail

# Demo: Open a channel, make off-chain payments, recipient withdraws, funder
# closes and refunds.
#
# Prerequisites:
# - stellar-cli installed
# - ed25519 installed (make install-tool-ed25519)
# - channel contract built (make build)

WASM="target/wasm32v1-none/release/channel.wasm"

echo "=== Setting up network ==="
stellar network use testnet

echo "=== Generating and funding identities ==="
stellar keys generate funder --fund 2>/dev/null || true
stellar keys generate recipient --fund 2>/dev/null || true
echo "Funder:    $(stellar keys address funder)"
echo "Recipient: $(stellar keys address recipient)"

echo ""
echo "=== Generating commitment key ==="
COMMITMENT_SKEY=$(ed25519 gen)
COMMITMENT_PKEY=$(ed25519 pub $COMMITMENT_SKEY)
echo "Commitment secret key: $COMMITMENT_SKEY"
echo "Commitment public key: $COMMITMENT_PKEY"

echo ""
echo "=== Deploying native asset contract ==="
stellar contract alias add native --id $(stellar contract id asset --asset native)
echo "Token: $(stellar contract id asset --asset native)"

echo ""
echo "=== Installing channel wasm ==="
WASM_HASH=$(stellar contract install \
    --wasm $WASM \
    --source funder)
echo "Wasm hash: $WASM_HASH"

echo ""
echo "=== Deploying channel contract ==="
stellar contract deploy \
    --alias channel \
    --wasm-hash $WASM_HASH \
    --source funder \
    -- \
    --token native \
    --from funder \
    --commitment_key $COMMITMENT_PKEY \
    --to recipient \
    --amount 10000000 \
    --refund_waiting_period 5
echo "Channel: $(stellar contract alias show channel)"

echo ""
echo "=== Channel state after open ==="
echo -n "Balance:   "
stellar contract invoke --id channel --send=no -- balance
echo -n "Deposited: "
stellar contract invoke --id channel --send=no -- deposited
echo -n "Withdrawn: "
stellar contract invoke --id channel --send=no -- withdrawn

echo ""
echo "=== Off-chain: Funder signs 4 commitments ==="

COMMITMENT_1=$(stellar contract invoke --id channel --send=no -- prepare_commitment --amount 1000000)
SIG_1=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_1)
echo "  Payment 1: cumulative 1,000,000 stroops, sig=$SIG_1"

COMMITMENT_2=$(stellar contract invoke --id channel --send=no -- prepare_commitment --amount 3000000)
SIG_2=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_2)
echo "  Payment 2: cumulative 3,000,000 stroops, sig=$SIG_2"

COMMITMENT_3=$(stellar contract invoke --id channel --send=no -- prepare_commitment --amount 6000000)
SIG_3=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_3)
echo "  Payment 3: cumulative 6,000,000 stroops, sig=$SIG_3"

COMMITMENT_4=$(stellar contract invoke --id channel --send=no -- prepare_commitment --amount 8000000)
SIG_4=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_4)
echo "  Payment 4: cumulative 8,000,000 stroops, sig=$SIG_4"

echo ""
echo "=== Recipient withdraws using commitment 3 (skipping 1 and 2) ==="
stellar keys use recipient
stellar contract invoke \
    --id channel \
    -- withdraw --amount 6000000 --sig $SIG_3

echo ""
echo "=== Channel state after first withdraw ==="
echo -n "Balance:   "
stellar contract invoke --id channel --send=no -- balance
echo -n "Deposited: "
stellar contract invoke --id channel --send=no -- deposited
echo -n "Withdrawn: "
stellar contract invoke --id channel --send=no -- withdrawn

echo ""
echo "=== Funder closes channel ==="
stellar keys use funder
stellar contract invoke \
    --id channel \
    -- close

echo ""
echo "=== Recipient withdraws using commitment 4 (during refund waiting period) ==="
stellar keys use recipient
stellar contract invoke \
    --id channel \
    -- withdraw --amount 8000000 --sig $SIG_4

echo ""
echo "=== Channel state after second withdraw ==="
echo -n "Balance:   "
stellar contract invoke --id channel --send=no -- balance
echo -n "Deposited: "
stellar contract invoke --id channel --send=no -- deposited
echo -n "Withdrawn: "
stellar contract invoke --id channel --send=no -- withdrawn

echo ""
echo "=== Funder refunds remainder (retrying until close is effective) ==="
stellar keys use funder
until stellar contract invoke --id channel -- refund 2>/dev/null; do
    echo "  Close not yet effective, retrying in 5s..."
    sleep 5
done

echo ""
echo "=== Channel state after refund ==="
echo -n "Balance:   "
stellar contract invoke --id channel --send=no -- balance
echo -n "Deposited: "
stellar contract invoke --id channel --send=no -- deposited
echo -n "Withdrawn: "
stellar contract invoke --id channel --send=no -- withdrawn

echo ""
echo "=== Done ==="
