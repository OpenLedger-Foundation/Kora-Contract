# Running Community Infrastructure for Kora Protocol

**Last Updated:** 2026-09-29  
**Target Audience:** Community members interested in running independent Kora infrastructure  
**Purpose:** Enable decentralized operation of RPC nodes and indexers to reduce reliance on centralized providers

---

## Table of Contents

1. [Overview](#overview)
2. [Why Run Community Infrastructure?](#why-run-community-infrastructure)
3. [System Requirements](#system-requirements)
4. [Prerequisites](#prerequisites)
5. [Running a Soroban RPC Node](#running-a-soroban-rpc-node)
6. [Running a Kora Indexer Instance](#running-a-kora-indexer-instance)
7. [Cross-Verification Against Canonical Instance](#cross-verification-against-canonical-instance)
8. [Monitoring and Maintenance](#monitoring-and-maintenance)
9. [Troubleshooting](#troubleshooting)
10. [Community Support](#community-support)

---

## Overview

Kora Protocol's indexer and backend services currently depend on a small number of RPC endpoints. Running your own infrastructure:
- **Improves resilience** against any single provider's downtime
- **Strengthens decentralization** by distributing trust across multiple operators
- **Provides data sovereignty** for applications building on Kora
- **Enables independent verification** of protocol state

This guide covers:
- ✅ Running a Soroban RPC node that consumes Stellar network data
- ✅ Running an independent Kora indexer instance from our open-source codebase
- ✅ Cross-verifying your indexer's output against the canonical instance
- ❌ Running Stellar Core validator nodes (out of scope; see [Stellar documentation](https://developers.stellar.org/docs/run-core-node))

---

## Why Run Community Infrastructure?

### Decentralization Benefits

**Current State:**
- Most Kora frontends query the canonical indexer at `https://indexer.kora.finance`
- This indexer relies on a small set of RPC providers (e.g., Stellar RPC, Infura, Alchemy)

**Community-Run Infrastructure Improves:**
1. **Censorship Resistance** — No single entity can block access to protocol data
2. **Liveness Guarantees** — Protocol remains queryable even if canonical indexer has downtime
3. **Geographic Distribution** — Reduced latency for users in underserved regions
4. **Trust Minimization** — Verify protocol state independently rather than trusting a single operator

### Use Cases

- **DApp Developers:** Run your own indexer to avoid rate limits and ensure uptime SLAs
- **Auditors/Researchers:** Independently verify protocol state and historical data
- **Regional Operators:** Provide low-latency infrastructure for users in Africa, LATAM, Asia
- **Institutional Users:** Meet internal requirements for data sovereignty and auditability

---

## System Requirements

### Minimum Specifications (Testnet)

| Component | Requirement |
|-----------|-------------|
| **CPU** | 4 cores (x86_64 or ARM64) |
| **RAM** | 8 GB |
| **Storage** | 100 GB SSD (grows ~10 GB/month) |
| **Network** | 100 Mbps sustained, 1 TB/month transfer |
| **OS** | Ubuntu 22.04 LTS, Debian 11+, or macOS 13+ |

### Recommended Specifications (Mainnet)

| Component | Requirement |
|-----------|-------------|
| **CPU** | 8 cores (x86_64) |
| **RAM** | 16 GB |
| **Storage** | 500 GB NVMe SSD (grows ~50 GB/month) |
| **Network** | 1 Gbps sustained, 5 TB/month transfer |
| **OS** | Ubuntu 22.04 LTS (preferred for production) |

**Cost Estimate:** ~$50-150/month on cloud providers (AWS c6i.2xlarge, Hetzner CX42, DigitalOcean)

---

## Prerequisites

### 1. Install Docker and Docker Compose

```bash
# Ubuntu/Debian
sudo apt update
sudo apt install -y docker.io docker-compose
sudo systemctl enable --now docker
sudo usermod -aG docker $USER
# Log out and back in for group changes to take effect

# Verify installation
docker --version
docker-compose --version
```

### 2. Install Rust (for building indexer from source)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
rustup default stable
```

### 3. Install Node.js (for verification tooling)

```bash
# Using nvm (recommended)
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.5/install.sh | bash
source ~/.bashrc
nvm install 18
nvm use 18
```

### 4. Clone Kora Repository

```bash
git clone https://github.com/kora-finance/contracts.git kora-protocol
cd kora-protocol
git checkout v0.1.0  # Use latest stable release
```

---

## Running a Soroban RPC Node

### Step 1: Install Stellar Quickstart (Recommended)

The easiest way to run a Soroban RPC node is via Stellar's Quickstart Docker image:

```bash
# Create data directory
mkdir -p ~/stellar-data

# Run Stellar Quickstart (connects to Stellar Testnet)
docker run --rm -it \
  --name stellar-quickstart \
  -p 8000:8000 \
  -p 11626:11626 \
  -v ~/stellar-data:/opt/stellar \
  stellar/quickstart:latest \
  --testnet

# For mainnet (Pubnet):
# Replace --testnet with --pubnet
```

**What This Does:**
- Runs a Stellar Core node (syncs Stellar blockchain)
- Runs a Soroban RPC server on `http://localhost:8000`
- Runs Horizon API (optional REST API) on `http://localhost:8000`

### Step 2: Wait for Initial Sync

```bash
# Check sync status
curl http://localhost:8000/health

# Expected output when synced:
# {"status": "healthy", "ledger": 123456}

# Monitor logs
docker logs -f stellar-quickstart
```

**Initial sync time:** 2-6 hours for testnet, 24-48 hours for mainnet (depends on network speed and disk I/O)

### Step 3: Verify RPC Endpoint

```bash
# Test Soroban RPC call
curl -X POST http://localhost:8000/soroban/rpc \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getHealth"
  }'

# Expected output:
# {"jsonrpc":"2.0","id":1,"result":{"status":"healthy"}}
```

### Alternative: Use Existing RPC Providers

If you don't want to run your own RPC node, you can use public providers:

**Testnet:**
- `https://soroban-testnet.stellar.org`
- `https://rpc-testnet.stellar.org`

**Mainnet:**
- `https://soroban-mainnet.stellar.org`
- `https://rpc-mainnet.stellar.org`

**Trade-offs:**
- ✅ No infrastructure maintenance
- ✅ Lower cost
- ❌ Rate limits apply
- ❌ Dependency on third-party availability
- ❌ Less decentralized

---

## Running a Kora Indexer Instance

### Step 1: Build the Indexer

```bash
cd services/indexer
cargo build --release

# Binary location: target/release/kora-indexer
```

### Step 2: Configure Environment Variables

Create `.env` file in `services/indexer/`:

```bash
# RPC endpoint (use your local node or public provider)
SOROBAN_RPC_URL=http://localhost:8000/soroban/rpc

# Network (testnet or mainnet)
STELLAR_NETWORK=testnet

# Contract addresses (get from Kora deployment)
CONTRACT_INVOICE_NFT=CCAI...
CONTRACT_MARKETPLACE=CCAM...
CONTRACT_FINANCING_POOL=CCAF...
CONTRACT_RISK_REGISTRY=CCAR...
CONTRACT_PRICE_ORACLE=CCAO...
CONTRACT_GOVERNANCE=CCAG...
CONTRACT_ACCESS_CONTROL=CCAA...

# Database (SQLite for local, PostgreSQL for production)
DATABASE_URL=sqlite://indexer.db

# Starting ledger (set to 0 to index from genesis, or latest for catchup)
START_LEDGER=0

# Checkpoint interval (how often to persist resume point)
CHECKPOINT_INTERVAL_SECONDS=60

# Event polling interval
POLL_INTERVAL_MS=1000

# Optional: Enable debug logging
RUST_LOG=info
```

**Getting Contract Addresses:**

```bash
# From Kora repository root
cat deployments/testnet/addresses.json
# Or for mainnet:
# cat deployments/mainnet/addresses.json
```

### Step 3: Initialize Database

```bash
# For SQLite (local/testing)
touch indexer.db

# For PostgreSQL (production)
createdb kora_indexer
psql kora_indexer < migrations/001_init.sql
```

### Step 4: Run the Indexer

```bash
# Development mode (with logs)
RUST_LOG=info cargo run --release

# Production mode (background daemon)
nohup cargo run --release > indexer.log 2>&1 &

# Using systemd (recommended for production)
sudo cp packaging/kora-indexer.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable kora-indexer
sudo systemctl start kora-indexer
```

### Step 5: Verify Indexing Progress

```bash
# Check logs
tail -f indexer.log

# Expected log output:
# [INFO] Indexer started, catching up from ledger 0
# [INFO] Indexed ledger 1000, checkpoint saved
# [INFO] Indexed ledger 2000, checkpoint saved
# ...

# Query indexed events (example with sqlite3)
sqlite3 indexer.db "SELECT COUNT(*) FROM events;"
sqlite3 indexer.db "SELECT * FROM events ORDER BY ledger_sequence DESC LIMIT 10;"
```

### Step 6: Expose Query API (Optional)

If you want to serve indexed data via REST API:

```bash
cd services/api/gateway
npm install
npm run build

# Configure to use your local indexer database
export DATABASE_URL=sqlite://../../indexer/indexer.db
export PORT=3000

npm start

# Test API
curl http://localhost:3000/api/v1/invoices
curl http://localhost:3000/api/v1/events?contract=invoice_nft&limit=10
```

---

## Cross-Verification Against Canonical Instance

### Purpose

Verify that your independent indexer produces identical results to the canonical instance, ensuring:
- ✅ Your RPC node is correctly synced
- ✅ Your indexer implementation matches the canonical version
- ✅ No data corruption or missed events

### Verification Tool

We provide `tools/verify-indexer.sh` to automate cross-verification:

```bash
#!/bin/bash
# tools/verify-indexer.sh
# Compares your local indexer output against canonical instance

set -e

CANONICAL_URL="${CANONICAL_URL:-https://indexer.kora.finance}"
LOCAL_URL="${LOCAL_URL:-http://localhost:3000}"
SAMPLE_SIZE="${SAMPLE_SIZE:-100}"

echo "Fetching $SAMPLE_SIZE recent events from canonical indexer..."
CANONICAL_EVENTS=$(curl -s "$CANONICAL_URL/api/v1/events?limit=$SAMPLE_SIZE")

echo "Fetching $SAMPLE_SIZE recent events from local indexer..."
LOCAL_EVENTS=$(curl -s "$LOCAL_URL/api/v1/events?limit=$SAMPLE_SIZE")

echo "Comparing event hashes..."
CANONICAL_HASH=$(echo "$CANONICAL_EVENTS" | jq -S '.' | sha256sum | awk '{print $1}')
LOCAL_HASH=$(echo "$LOCAL_EVENTS" | jq -S '.' | sha256sum | awk '{print $1}')

if [ "$CANONICAL_HASH" == "$LOCAL_HASH" ]; then
    echo "✅ PASS: Local indexer matches canonical instance"
    echo "   Hash: $LOCAL_HASH"
    exit 0
else
    echo "❌ FAIL: Local indexer diverges from canonical instance"
    echo "   Canonical hash: $CANONICAL_HASH"
    echo "   Local hash:     $LOCAL_HASH"
    echo ""
    echo "Showing first difference:"
    diff <(echo "$CANONICAL_EVENTS" | jq -S '.') <(echo "$LOCAL_EVENTS" | jq -S '.') | head -20
    exit 1
fi
```

### Running Verification

```bash
chmod +x tools/verify-indexer.sh

# Verify against testnet canonical
CANONICAL_URL=https://indexer-testnet.kora.finance \
LOCAL_URL=http://localhost:3000 \
  ./tools/verify-indexer.sh

# Verify against mainnet canonical (when running mainnet node)
CANONICAL_URL=https://indexer.kora.finance \
LOCAL_URL=http://localhost:3000 \
  ./tools/verify-indexer.sh
```

### Interpreting Results

**✅ PASS:** Your indexer is correctly synced and matches canonical state.

**❌ FAIL — Possible Causes:**
1. **Ledger gap:** Your RPC node is behind; wait for sync to complete
2. **Configuration mismatch:** Verify contract addresses in `.env` match canonical deployment
3. **Software version mismatch:** Ensure you're running the same indexer version as canonical (check `git describe --tags`)
4. **Database corruption:** Drop database and re-index from genesis

### Continuous Verification (Recommended)

Set up automated verification via cron:

```bash
# Add to crontab (run verification every 6 hours)
crontab -e

0 */6 * * * /home/user/kora-protocol/tools/verify-indexer.sh >> /var/log/kora-verify.log 2>&1
```

---

## Monitoring and Maintenance

### Key Metrics to Monitor

| Metric | Target | Alert Threshold |
|--------|--------|-----------------|
| **Indexer Lag** | <60 seconds behind chain tip | >300 seconds |
| **Events Indexed/Second** | 10-50 (varies by activity) | <1 (stalled) |
| **Database Size Growth** | ~10 GB/month (testnet), ~50 GB/month (mainnet) | >2x expected |
| **RPC Node Sync Status** | `healthy` | `syncing` for >1 hour |
| **Disk Usage** | <80% | >90% |
| **Memory Usage** | <80% | >90% |

### Monitoring Commands

```bash
# Check indexer lag
curl http://localhost:3000/api/v1/health
# {"status":"healthy","last_indexed_ledger":123456,"chain_tip":123460,"lag_seconds":40}

# Check RPC node health
curl http://localhost:8000/health

# Aggregated health (schemaVersion 1.1.0; 503 when critical probes fail)
curl http://localhost:8080/health
# Probes: reporting GET $REPORTING_URL (default http://reporting:8080/health),
# ipfs-pinning GET $IPFS_PINNING_URL (default http://ipfs-pinning:8080/health).
# Reporting export queue backup surfaces "degraded"; all pinning providers
# failing surfaces "degraded"/"down". Probe timeouts count as failures.

# Check disk usage
df -h

# Check memory usage
free -h

# Check indexer process
ps aux | grep kora-indexer
systemctl status kora-indexer
```

### Automated Alerting (Prometheus + Grafana)

Example Prometheus scrape config:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'kora-indexer'
    static_configs:
      - targets: ['localhost:9090']  # Assuming indexer exposes metrics endpoint

  - job_name: 'stellar-rpc'
    static_configs:
      - targets: ['localhost:8000']
```

### Log Rotation

```bash
# Configure logrotate for indexer logs
sudo nano /etc/logrotate.d/kora-indexer

/home/user/kora-protocol/services/indexer/indexer.log {
    daily
    rotate 7
    compress
    missingok
    notifempty
    copytruncate
}
```

### Backup Strategy

```bash
# Backup indexer database (SQLite example)
sqlite3 indexer.db ".backup indexer_backup_$(date +%Y%m%d).db"

# Upload to S3 (optional)
aws s3 cp indexer_backup_*.db s3://my-kora-backups/

# PostgreSQL backup
pg_dump kora_indexer > kora_indexer_$(date +%Y%m%d).sql
```

**Note:** Indexer data is reconstructable from RPC node; backups are optional for convenience (avoids re-indexing on failure).

---

## Troubleshooting

### Issue: Indexer Not Catching Up

**Symptoms:** `last_indexed_ledger` not increasing, logs show no activity

**Causes & Solutions:**

1. **RPC node not synced:**
   ```bash
   curl http://localhost:8000/health
   # If status != "healthy", wait for RPC node to finish syncing
   ```

2. **Invalid contract addresses:**
   ```bash
   # Verify addresses match deployment
   cat deployments/testnet/addresses.json
   # Update .env if mismatched
   ```

3. **Database lock (SQLite):**
   ```bash
   # Kill any stale processes
   pkill kora-indexer
   # Restart indexer
   systemctl restart kora-indexer
   ```

### Issue: Verification Fails with Hash Mismatch

**Causes & Solutions:**

1. **Indexer version mismatch:**
   ```bash
   git describe --tags
   # Ensure matches canonical: https://indexer.kora.finance/version
   ```

2. **Incomplete sync:**
   ```bash
   # Check if caught up to chain tip
   curl http://localhost:3000/api/v1/health
   ```

3. **Event parsing bug:**
   ```bash
   # Check logs for decode errors
   tail -f indexer.log | grep ERROR
   # Report to GitHub: https://github.com/kora-finance/contracts/issues
   ```

### Issue: High Memory Usage

**Causes & Solutions:**

1. **Event cache too large:**
   ```bash
   # Reduce CHECKPOINT_INTERVAL_SECONDS in .env
   CHECKPOINT_INTERVAL_SECONDS=30  # Flush more frequently
   ```

2. **PostgreSQL connection leak:**
   ```bash
   # Check active connections
   psql kora_indexer -c "SELECT count(*) FROM pg_stat_activity;"
   # Restart indexer if >100 connections
   ```

### Issue: RPC Node Disk Full

**Solutions:**

1. **Enable automatic pruning:**
   ```bash
   # Edit stellar-core.cfg
   AUTOMATIC_MAINTENANCE_PERIOD=14400  # Prune every 4 hours
   AUTOMATIC_MAINTENANCE_COUNT=5       # Keep last 5 ledgers
   ```

2. **Manually prune old ledgers:**
   ```bash
   stellar-core --c stellar-core.cfg --ll INFO prune-ledgers 100000
   ```

### Issue: Indexer Crashes on Startup

**Check logs for specific error:**

```bash
tail -100 indexer.log

# Common errors:
# - "Address already in use" → Kill old process: pkill kora-indexer
# - "Database locked" → Remove lock file: rm indexer.db-lock
# - "Invalid CONTRACT_* address" → Verify addresses in .env
```

---

## Community Support

### Resources

- **Documentation:** [https://docs.kora.finance/infrastructure](https://docs.kora.finance/infrastructure)
- **GitHub Issues:** [https://github.com/kora-finance/contracts/issues](https://github.com/kora-finance/contracts/issues)
- **Discord:** [https://discord.gg/kora-finance](https://discord.gg/kora-finance) (#infrastructure channel)
- **Community Forum:** [https://forum.kora.finance](https://forum.kora.finance)

### Reporting Issues

When reporting infrastructure issues, include:

1. **System info:** OS, CPU, RAM, disk space
2. **Software versions:** `git describe --tags`, `docker --version`, `rustc --version`
3. **Configuration:** Sanitized `.env` file (remove any secrets)
4. **Logs:** Last 100 lines of indexer.log and RPC node logs
5. **Error message:** Full error text and stack trace if applicable

**Template:**

```
## Environment
- OS: Ubuntu 22.04
- CPU: 4 cores
- RAM: 8 GB
- Indexer version: v0.1.0
- RPC provider: Self-hosted (Stellar Quickstart)

## Issue Description
Indexer stalls at ledger 50000, no new events indexed for 2 hours.

## Logs
<paste last 100 lines of indexer.log>

## Steps Taken
1. Restarted indexer service
2. Verified RPC node is synced (health check returns "healthy")
3. Checked disk space (200 GB free)
```

### Contributing Infrastructure Improvements

Community contributions welcome! See [CONTRIBUTING.md](../CONTRIBUTING.md) for guidelines.

**High-Impact Contributions:**
- Performance optimizations for indexer throughput
- Support for additional databases (MySQL, MongoDB)
- Monitoring dashboard templates (Grafana, DataDog)
- Deployment automation (Terraform, Ansible, Kubernetes)
- Documentation improvements based on real-world experience

---

## Appendix: Production Deployment Checklist

Before running in production:

- [ ] Hardware meets recommended specifications
- [ ] RPC node fully synced and health check passing
- [ ] Indexer verified against canonical instance (3+ consecutive passes)
- [ ] Monitoring and alerting configured (Prometheus + Grafana or equivalent)
- [ ] Log rotation configured
- [ ] Database backups configured (optional but recommended)
- [ ] Firewall rules configured (only expose necessary ports)
- [ ] systemd service configured for auto-restart
- [ ] Disk space growth projections validated (50 GB/month mainnet)
- [ ] Documented runbook for common issues
- [ ] On-call rotation established (if serving production traffic)

---

## Version History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-09-29 | Initial guide for community infrastructure |

**Next Review:** 2027-03-29 (6 months)

---

**This guide is consistent with `docs/RELEASE.md` and supersedes any conflicting informal documentation.**
