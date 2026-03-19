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
echo "=== Deploying channel contract ==="
stellar contract deploy \
    --alias channel \
    --wasm $WASM \
    --source funder \
    -- \
    --token native \
    --from funder \
    --commitment_key $COMMITMENT_PKEY \
    --to recipient \
    --amount 10000000 \
    --refund_waiting_period 10
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
echo "=== Off-chain: Funder signs commitments ==="
# Payment 1: authorize recipient to withdraw up to 3,000,000 stroops.
COMMITMENT_1=$(stellar contract invoke \
    --id channel --send=no \
    -- prepare_commitment --amount 3000000)
# Remove quotes from the output.
COMMITMENT_1=$(echo $COMMITMENT_1 | tr -d '"')
echo "Commitment 1 (3000000): $COMMITMENT_1"
SIG_1=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_1)
echo "Signature 1: $SIG_1"

# Payment 2: authorize recipient to withdraw up to 7,000,000 stroops.
COMMITMENT_2=$(stellar contract invoke \
    --id channel --send=no \
    -- prepare_commitment --amount 7000000)
COMMITMENT_2=$(echo $COMMITMENT_2 | tr -d '"')
echo "Commitment 2 (7000000): $COMMITMENT_2"
SIG_2=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_2)
echo "Signature 2: $SIG_2"

echo ""
echo "=== Recipient withdraws using commitment 2 (latest) ==="
stellar keys use recipient
stellar contract invoke \
    --id channel \
    -- withdraw --amount 7000000 --sig $SIG_2

echo ""
echo "=== Channel state after withdraw ==="
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
echo "=== Funder refunds remainder (retrying until close is effective) ==="
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
