# =============================================================================
# Kora Protocol — Makefile
# =============================================================================

.PHONY: build test clean fmt lint check audit coverage deploy-testnet deploy-mainnet fuzz fuzz-deep help build-optimized test-verbose setup sizes

WASM_TARGET := wasm32-unknown-unknown
CONTRACTS   := access_control invoice_nft marketplace financing_pool treasury risk_registry
COVERAGE_MIN ?= 95

# Fuzzing knobs (see contracts/fuzz/README.md)
FUZZ_ITERS  ?= 10000
FUZZ_TARGET ?= marketplace
FUZZ_RUNS   ?= 1000000

# ── Help ──────────────────────────────────────────────────────────────────────

help:
	@echo "Kora Protocol — Available Targets:"
	@echo ""
	@echo "Build targets:"
	@echo "  build                Build all contracts for wasm32-unknown-unknown"
	@echo "  build-optimized      Build and optimize all WASM contracts"
	@echo ""
	@echo "Test targets:"
	@echo "  test                 Run all tests (unit and integration)"
	@echo "  test-verbose         Run tests with output capture disabled"
	@echo "  fuzz                 Run deterministic fuzz tests with seed corpus"
	@echo "  fuzz-deep            Run libFuzzer deep fuzz testing (nightly required)"
	@echo "  coverage             Run code coverage analysis (requires cargo-tarpaulin)"
	@echo ""
	@echo "Code quality targets:"
	@echo "  fmt                  Format all Rust code using cargo fmt"
	@echo "  lint                 Run clippy linter with strict warnings-as-errors"
	@echo "  check                Run cargo check on all packages"
	@echo "  audit                Run cargo-deny and cargo-audit for supply chain security"
	@echo ""
	@echo "Deployment targets:"
	@echo "  deploy-testnet       Build optimized and deploy to Stellar testnet"
	@echo "  deploy-mainnet       Build optimized and deploy to Stellar mainnet (confirmation required)"
	@echo ""
	@echo "Utility targets:"
	@echo "  setup                Install Rust target and required tools"
	@echo "  sizes                Display WASM artifact sizes for all contracts"
	@echo "  clean                Remove all build artifacts"
	@echo "  help                 Display this help message"
	@echo ""
	@echo "Usage: make <target> [VARIABLE=value ...]"
	@echo ""
	@echo "Examples:"
	@echo "  make test"
	@echo "  make fuzz FUZZ_ITERS=50000"
	@echo "  make coverage COVERAGE_MIN=85"

# ── Build ─────────────────────────────────────────────────────────────────────

build:
	cargo build --target $(WASM_TARGET) --release

build-optimized: build
	@for c in $(CONTRACTS); do \
		wasm="target/$(WASM_TARGET)/release/kora_$${c}.wasm"; \
		if [ -f "$$wasm" ]; then \
			stellar contract optimize --wasm "$$wasm"; \
			echo "Optimized: $$wasm"; \
		fi; \
	done

# ── Test ──────────────────────────────────────────────────────────────────────

test:
	cargo test --all

test-verbose:
	cargo test --all -- --nocapture

# ── Code Quality ──────────────────────────────────────────────────────────────

fmt:
	cargo fmt --all

lint:
	cargo clippy --all -- -D warnings

check:
	cargo check --all

# ── Fuzz ──────────────────────────────────────────────────────────────────────

# Deterministic smoke harness on stable Rust: FUZZ_ITERS seeded iterations per
# contract plus the checked-in seed corpus. This is the CI fuzz check.
#   make fuzz FUZZ_ITERS=50000
fuzz:
	FUZZ_ITERS=$(FUZZ_ITERS) cargo test -p kora-fuzz --test smoke -- --ignored --nocapture

# libFuzzer deep run for one target. Needs nightly + `cargo install cargo-fuzz`.
#   make fuzz-deep FUZZ_TARGET=marketplace FUZZ_RUNS=5000000
fuzz-deep:
	cd contracts/fuzz/fuzz && cargo +nightly fuzz run fuzz_$(FUZZ_TARGET) -- -runs=$(FUZZ_RUNS)

# ── Audit ─────────────────────────────────────────────────────────────────────
#
# Run locally to replicate the `supply-chain-audit` CI gate (issue #609).
# Requires:  cargo install cargo-deny --locked --version 0.14.24
#            cargo install cargo-audit --locked --version 0.21.0
# Exception process: see deny.toml and docs/INCIDENT_RESPONSE.md §7b.

audit:
	cargo deny check
	cargo audit --deny warnings

coverage:
	@echo "Running coverage analysis (threshold: $(COVERAGE_MIN)%)..."
	cargo tarpaulin --all --timeout 300 --out Stdout | tee /tmp/coverage.txt
	@coverage=$$(grep -oP 'Coverage: \K[0-9.]+' /tmp/coverage.txt | head -1); \
	if [ -z "$$coverage" ]; then \
		echo "Error: Could not parse coverage percentage"; \
		exit 1; \
	fi; \
	if (( $$(echo "$$coverage < $(COVERAGE_MIN)" | bc -l) )); then \
		echo "Coverage $$coverage% is below threshold of $(COVERAGE_MIN)%"; \
		exit 1; \
	else \
		echo "Coverage $$coverage% meets threshold of $(COVERAGE_MIN)%"; \
	fi

# ── Clean ─────────────────────────────────────────────────────────────────────

clean:
	cargo clean

# ── Deploy ────────────────────────────────────────────────────────────────────

deploy-testnet: build-optimized
	bash scripts/deploy.sh testnet

deploy-mainnet: build-optimized
	@echo "WARNING: Deploying to MAINNET. Press Ctrl+C to abort, Enter to continue."
	@read _
	bash scripts/deploy.sh mainnet

# ── Helpers ───────────────────────────────────────────────────────────────────

setup:
	rustup target add $(WASM_TARGET)
	cargo install stellar-cli --locked

sizes: build
	@echo "WASM sizes:"
	@for c in $(CONTRACTS); do \
		wasm="target/$(WASM_TARGET)/release/kora_$${c}.wasm"; \
		if [ -f "$$wasm" ]; then \
			printf "  %-25s %s\n" "$$c" "$$(du -sh $$wasm | cut -f1)"; \
		fi; \
	done
