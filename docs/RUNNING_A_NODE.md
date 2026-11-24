# Running a Kora Deploy Node (issue #1082)

This guide covers the full workflow for deploy operators, including upgrading a
live environment using the `bootstrap-registry` helper.

---

## Prerequisites

- `stellar` CLI installed and on `PATH`.
- `DEPLOYER_SECRET` environment variable set to your deploy key or Stellar CLI alias.
- Pre-built contract WASMs: run `make build` from the workspace root.
- Access to the deployment manifest at `deployments/<network>.json`.

---

## Initial Deployment

Follow `scripts/deploy.sh` for the initial deployment order. The registry must
be deployed and initialized **before** `marketplace` and `financing_pool`, since
both read governance-enacted parameters from it at runtime.

```bash
export DEPLOYER_SECRET=<your-key>
./scripts/deploy.sh testnet
```

After deployment, the manifest is written to `deployments/<network>.json` and a
versioned copy to `deployments/manifests/<network>-<timestamp>-<git-sha>.json`.

---

## Upgrading a Live Environment

### Step 1 — Identify the existing registry address

```bash
REGISTRY_ID=$(jq -r '.contracts.parameter_registry.address' deployments/testnet.json)
echo "Registry: $REGISTRY_ID"
```

### Step 2 — Dry-run: inspect current on-chain parameters

```bash
cargo run -p kora-xtask --bin bootstrap-registry -- \
  --contract "$REGISTRY_ID" \
  --network testnet
```

This prints a JSON document with every parameter's current value and whether it
was enacted by a governance proposal (`"governed": true`) or is still a
bootstrap default (`"governed": false`).

```json
{
  "contract": "CABC...",
  "network": "testnet",
  "fetched_at": 1700000042,
  "params": {
    "MarketplaceFeeBps": {"value": 50, "governed": false},
    "DebtorAttestationB": {"value": 1, "governed": false},
    ...
  }
}
```

### Step 3 — Generate apply commands (before wiring multisig)

> **WARNING**: `admin_set_parameter` is the **bootstrap** write path. It is
> permanently blocked once you configure a multisig threshold on
> `access_control`. Run the apply script **only** on a fresh deployment or
> before wiring governance signers.

```bash
cargo run -p kora-xtask --bin bootstrap-registry -- \
  --contract "$REGISTRY_ID" \
  --network testnet \
  --apply > /tmp/apply-params.sh
```

Review `/tmp/apply-params.sh`, then run it:

```bash
export DEPLOYER_SECRET=<your-key>
export ADMIN=$(stellar keys address "$DEPLOYER_SECRET")
bash /tmp/apply-params.sh
```

Governed parameters (those with `"governed": true` in the dry-run output) are
labelled `# SKIP` and are not emitted as commands. To change a governed
parameter, propose an update through the governance flow:

```bash
stellar contract invoke --id "$REGISTRY_ID" ... \
  -- propose_param_update --proposer "$ADMIN" \
  --key "MarketplaceFeeBps" --new_value 75
```

### Step 4 — Verify

Re-run the dry-run to confirm all values match your expectations before wiring
governance signers:

```bash
cargo run -p kora-xtask --bin bootstrap-registry -- \
  --contract "$REGISTRY_ID" \
  --network testnet
```

---

## Edge Cases

### Missing contract address

If `--contract` is not supplied or the manifest file does not contain the
registry address, the tool exits with code 1 and prints a hint:

```
error: --contract <CONTRACT_ID> is required
  If --contract is not known, check the deployment manifest at
  deployments/<network>.json (field 'contracts.parameter_registry.address').
```

### Partial storage after upgrade

When a new registry WASM adds parameters that were not present in the previous
version, those keys will be absent from storage. The tool reports them as `0`
with a warning:

```
WARNING: could not read key 'SomeNewKey' from registry (network error); using 0
WARNING: 1 key(s) could not be read; zero values used for those keys.
         This may indicate partial storage after an upgrade — verify manually.
```

Verify the expected value from the contract source, then seed the key with
`admin_set_parameter` or via governance proposal before dependents read it.

---

## Deployment dependency graph

```
access_control
    └─► parameter_registry     ← must be deployed and seeded first
        └─► invoice_nft
        └─► risk_registry
        └─► dispute_resolution
        └─► debtor_attestation
            └─► financing_pool
                └─► marketplace   ← reads registry at list_invoice / fund_invoice
```

`scripts/deploy.sh` enforces this order. See [`docs/CONTRACTS.md`](CONTRACTS.md)
for the full contract reference.

---

## Community Node (indexer + API + health)

```bash
cp packaging/.env.example .env   # fill in CONTRACT_* and TREASURY_CONTRACT_ID
./packaging/community-node.sh
```

Starts Postgres, the indexer (`:3000`), the treasury API (`:3001`) and the health
aggregator (`http://localhost:8081/health`), and waits until all are healthy.
Re-running is safe. Missing configuration exits 2 before any container starts;
a degraded or failed dependency fails the wait. Set `KORA_INDEXER_IMAGE` to run a
prebuilt indexer. `services/indexer` currently ships only a library, with no
server binary, so a local indexer build requires one to be added first.
`HEALTH_SERVICES` limits which upstreams the aggregator polls.
