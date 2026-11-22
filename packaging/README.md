# Kora Indexer Packaging & Deployment

This directory contains packaging and deployment automation for community-run Kora indexer instances.

## Contents

- **`kora-indexer.service`** — systemd service definition for production deployment
- **`deploy-indexer.sh`** — Automated deployment script (requires sudo)
- **`Dockerfile`** — Multi-stage Docker image for the indexer binary
- **`docker-compose.yml`** — Docker Compose stack (indexer + Postgres)
- **`.env.example`** — Environment variable template for both deployment paths

## Quick Start

### Docker Compose (recommended)

The fastest way to run a community indexer with a local Postgres database.

```bash
# 1. Copy and edit the env template
cp packaging/.env.example .env
# Fill in STELLAR_NETWORK, SOROBAN_RPC_URL, and CONTRACT_* addresses

# 2. Start the stack
docker compose -f packaging/docker-compose.yml up -d

# 3. Tail logs
docker compose -f packaging/docker-compose.yml logs -f indexer
```

Health check: `curl http://localhost:3000/api/v1/health`

### Docker (standalone)

```bash
# Build the image from the workspace root
docker build -t kora-indexer:latest -f packaging/Dockerfile .

# Run with an external Postgres or SQLite
docker run --env-file .env -p 3000:3000 kora-indexer:latest
```

### Automated Deployment (Ubuntu/Debian)

```bash
# Clone repository
git clone https://github.com/kora-finance/contracts.git
cd contracts

# Run deployment script (requires sudo)
sudo ./packaging/deploy-indexer.sh testnet

# Edit contract addresses
sudo nano /opt/kora/services/indexer/.env

# Start service
sudo systemctl start kora-indexer
sudo systemctl status kora-indexer
```

### Manual Deployment

See **[docs/RUNNING_A_NODE.md](../docs/RUNNING_A_NODE.md)** for step-by-step manual installation.

## Service Management

```bash
# Start indexer
sudo systemctl start kora-indexer

# Stop indexer
sudo systemctl stop kora-indexer

# Restart indexer
sudo systemctl restart kora-indexer

# View status
sudo systemctl status kora-indexer

# View logs
sudo journalctl -u kora-indexer -f

# Enable auto-start on boot
sudo systemctl enable kora-indexer

# Disable auto-start
sudo systemctl disable kora-indexer
```

## Configuration

Service configuration is loaded from `/opt/kora/services/indexer/.env`.

Edit configuration:
```bash
sudo nano /opt/kora/services/indexer/.env
sudo systemctl restart kora-indexer
```

## Verification

After deployment, verify your indexer matches the canonical instance:

```bash
cd /opt/kora
./tools/verify-indexer.sh
```

## Resource Limits

The systemd service includes default resource limits:
- **Memory:** 2 GB maximum
- **CPU:** 200% (2 cores)
- **File descriptors:** 65536

Adjust limits in `/etc/systemd/system/kora-indexer.service`:

```bash
sudo nano /etc/systemd/system/kora-indexer.service
# Edit MemoryMax= and CPUQuota= values
sudo systemctl daemon-reload
sudo systemctl restart kora-indexer
```

## Monitoring

### Health Check

```bash
curl http://localhost:3000/api/v1/health
```

### Metrics Export (Prometheus)

Coming soon (track issue #XXX)

## Upgrading

```bash
# Stop service
sudo systemctl stop kora-indexer

# Pull latest code
cd /opt/kora
sudo -u kora git pull origin main
sudo -u kora git checkout $(git describe --tags --abbrev=0)

# Rebuild
cd services/indexer
sudo -u kora cargo build --release

# Restart service
sudo systemctl start kora-indexer
```

## Backup & Recovery

### Database Backup

```bash
# Backup SQLite database
sudo -u kora sqlite3 /opt/kora/services/indexer/indexer.db \
  ".backup /opt/kora/backups/indexer_$(date +%Y%m%d).db"
```

### Recovery from Backup

```bash
# Stop service
sudo systemctl stop kora-indexer

# Restore backup
sudo -u kora cp /opt/kora/backups/indexer_20260929.db \
  /opt/kora/services/indexer/indexer.db

# Start service
sudo systemctl start kora-indexer
```

### Re-index from Scratch

```bash
# Stop service
sudo systemctl stop kora-indexer

# Remove database
sudo rm /opt/kora/services/indexer/indexer.db

# Start service (will re-index from START_LEDGER in .env)
sudo systemctl start kora-indexer
```

## Troubleshooting

See **[docs/RUNNING_A_NODE.md#troubleshooting](../docs/RUNNING_A_NODE.md#troubleshooting)** for common issues and solutions.

### Quick Diagnostics

```bash
# Check if service is running
sudo systemctl status kora-indexer

# Check recent logs for errors
sudo journalctl -u kora-indexer -n 100 --no-pager

# Check RPC connectivity
curl http://localhost:8000/health

# Check database file exists
ls -lh /opt/kora/services/indexer/indexer.db

# Check disk space
df -h /opt/kora

# Check memory usage
free -h
```

## Support

- **Documentation:** [docs/RUNNING_A_NODE.md](../docs/RUNNING_A_NODE.md)
- **Issues:** https://github.com/kora-finance/contracts/issues
- **Discord:** https://discord.gg/kora-finance (#infrastructure)

## Security

- Service runs as unprivileged `kora` user
- Filesystem access restricted via systemd security directives
- No network-facing ports by default (API gateway optional)
- Environment variables isolated in `/opt/kora/services/indexer/.env` (mode 600)

For security issues, see [SECURITY.md](../SECURITY.md).
