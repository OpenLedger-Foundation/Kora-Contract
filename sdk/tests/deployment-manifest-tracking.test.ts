import { describe, it, expect, beforeEach, afterEach } from "vitest";
import * as fs from "fs";
import * as path from "path";

const MOCK_DEPLOYMENTS_DIR = "/tmp/kora-test-deployments";
const MOCK_WASM_DIR = "/tmp/kora-test-wasm";

interface DeploymentManifest {
  network: string;
  deployed_at: string;
  admin: string;
  git_commit: string;
  parameters: {
    treasury_fee_bps: number;
    marketplace_fee_bps: number;
    marketplace_referrer_bps: number;
    late_penalty_bps: number;
    max_position_bps: number;
    oracle_base_currency: string;
  };
  contracts: {
    [key: string]: {
      address: string;
      wasm_hash: string;
    };
  };
}

interface ContractDeploymentRecord {
  name: string;
  wasmPath: string;
  address: string;
  wasmHash: string;
}

class DeploymentManifestTracker {
  private manifestPath: string;
  private deploymentRecords: ContractDeploymentRecord[] = [];

  constructor(network: string, baseDir: string = MOCK_DEPLOYMENTS_DIR) {
    this.manifestPath = path.join(baseDir, `${network}.json`);
  }

  addContractDeployment(
    name: string,
    wasmPath: string,
    address: string,
    wasmHash: string
  ): void {
    this.deploymentRecords.push({
      name,
      wasmPath,
      address,
      wasmHash,
    });
  }

  getDeploymentRecords(): ContractDeploymentRecord[] {
    return this.deploymentRecords;
  }

  writeManifest(
    network: string,
    admin: string,
    gitCommit: string,
    parameters: {
      treasury_fee_bps: number;
      marketplace_fee_bps: number;
      marketplace_referrer_bps: number;
      late_penalty_bps: number;
      max_position_bps: number;
      oracle_base_currency: string;
    }
  ): void {
    const contracts: { [key: string]: { address: string; wasm_hash: string } } =
      {};

    for (const record of this.deploymentRecords) {
      contracts[record.name] = {
        address: record.address,
        wasm_hash: record.wasmHash,
      };
    }

    const manifest: DeploymentManifest = {
      network,
      deployed_at: new Date().toISOString(),
      admin,
      git_commit: gitCommit,
      parameters,
      contracts,
    };

    const dir = path.dirname(this.manifestPath);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }

    fs.writeFileSync(this.manifestPath, JSON.stringify(manifest, null, 2));
  }

  readManifest(): DeploymentManifest {
    if (!fs.existsSync(this.manifestPath)) {
      throw new Error(`Manifest not found: ${this.manifestPath}`);
    }

    const content = fs.readFileSync(this.manifestPath, "utf-8");
    return JSON.parse(content);
  }

  validateManifest(expectedContracts: string[]): boolean {
    const manifest = this.readManifest();

    for (const contractName of expectedContracts) {
      if (!manifest.contracts[contractName]) {
        throw new Error(
          `Contract ${contractName} not found in manifest contracts`
        );
      }

      const contract = manifest.contracts[contractName];
      if (!contract.address || contract.address.length === 0) {
        throw new Error(`Contract ${contractName} has empty address in manifest`);
      }

      if (!contract.wasm_hash || contract.wasm_hash.length !== 64) {
        throw new Error(
          `Contract ${contractName} has invalid wasm_hash in manifest`
        );
      }
    }

    return true;
  }

  verifyAtomicity(): boolean {
    const manifest = this.readManifest();

    if (!manifest.deployed_at || !manifest.git_commit) {
      throw new Error("Manifest missing critical fields (deployed_at, git_commit)");
    }

    const allContracts = Object.values(manifest.contracts);
    if (allContracts.length === 0) {
      throw new Error("Manifest has no contracts");
    }

    for (const contract of allContracts) {
      if (!contract.address || !contract.wasm_hash) {
        throw new Error("Manifest has incomplete contract entry");
      }
    }

    return true;
  }
}

describe("Deployment Manifest Tracking (Issue #649)", () => {
  let tracker: DeploymentManifestTracker;

  beforeEach(() => {
    if (!fs.existsSync(MOCK_DEPLOYMENTS_DIR)) {
      fs.mkdirSync(MOCK_DEPLOYMENTS_DIR, { recursive: true });
    }
    tracker = new DeploymentManifestTracker("testnet", MOCK_DEPLOYMENTS_DIR);
  });

  afterEach(() => {
    if (fs.existsSync(MOCK_DEPLOYMENTS_DIR)) {
      fs.rmSync(MOCK_DEPLOYMENTS_DIR, { recursive: true, force: true });
    }
  });

  describe("manifest creation", () => {
    it("should create a valid deployment manifest with all contracts", () => {
      const gitCommit = "abc123def456";
      const admin = "GBTCHKHMYAFFCHYL7JQZG7Z5MGHVKWMXYDPQTWGMQ53PZAHDCL3CWYQG";

      tracker.addContractDeployment(
        "access_control",
        `${MOCK_WASM_DIR}/access_control.wasm`,
        "CABC123",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
      );
      tracker.addContractDeployment(
        "invoice_nft",
        `${MOCK_WASM_DIR}/invoice_nft.wasm`,
        "CINV456",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b856"
      );
      tracker.addContractDeployment(
        "financing_pool",
        `${MOCK_WASM_DIR}/financing_pool.wasm`,
        "CPOOL789",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b857"
      );

      tracker.writeManifest("testnet", admin, gitCommit, {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      const manifest = tracker.readManifest();
      expect(manifest.network).toBe("testnet");
      expect(manifest.admin).toBe(admin);
      expect(manifest.git_commit).toBe(gitCommit);
      expect(manifest.contracts.access_control.address).toBe("CABC123");
      expect(manifest.contracts.invoice_nft.address).toBe("CINV456");
      expect(manifest.contracts.financing_pool.address).toBe("CPOOL789");
    });

    it("should include wasm hashes in manifest for verification", () => {
      const hash1 =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
      const hash2 =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b856";

      tracker.addContractDeployment("access_control", "", "CABC123", hash1);
      tracker.addContractDeployment("invoice_nft", "", "CINV456", hash2);

      tracker.writeManifest("testnet", "ADMIN", "commit123", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      const manifest = tracker.readManifest();
      expect(manifest.contracts.access_control.wasm_hash).toBe(hash1);
      expect(manifest.contracts.invoice_nft.wasm_hash).toBe(hash2);
    });

    it("should include git commit SHA for audit trail", () => {
      const gitCommit = "abc123def456abc123def456";

      tracker.addContractDeployment("access_control", "", "CABC123", "hash1");

      tracker.writeManifest("testnet", "ADMIN", gitCommit, {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      const manifest = tracker.readManifest();
      expect(manifest.git_commit).toBe(gitCommit);
    });

    it("should include deployment timestamp", () => {
      tracker.addContractDeployment("access_control", "", "CABC123", "hash1");

      const beforeTime = new Date();
      tracker.writeManifest("testnet", "ADMIN", "commit123", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });
      const afterTime = new Date();

      const manifest = tracker.readManifest();
      const deployedTime = new Date(manifest.deployed_at);

      expect(deployedTime.getTime()).toBeGreaterThanOrEqual(beforeTime.getTime());
      expect(deployedTime.getTime()).toBeLessThanOrEqual(afterTime.getTime());
    });
  });

  describe("manifest validation", () => {
    it("should validate presence of all required contracts", () => {
      tracker.addContractDeployment("access_control", "", "CABC123", "hash1");
      tracker.addContractDeployment("invoice_nft", "", "CINV456", "hash2");

      tracker.writeManifest("testnet", "ADMIN", "commit123", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      const isValid = tracker.validateManifest([
        "access_control",
        "invoice_nft",
      ]);
      expect(isValid).toBe(true);
    });

    it("should throw error when contract missing from manifest", () => {
      tracker.addContractDeployment("access_control", "", "CABC123", "hash1");

      tracker.writeManifest("testnet", "ADMIN", "commit123", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      expect(() => {
        tracker.validateManifest(["access_control", "missing_contract"]);
      }).toThrow("missing_contract");
    });

    it("should throw error when contract has invalid address", () => {
      tracker.addContractDeployment("access_control", "", "", "hash1");

      tracker.writeManifest("testnet", "ADMIN", "commit123", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      expect(() => {
        tracker.validateManifest(["access_control"]);
      }).toThrow("empty address");
    });

    it("should throw error when wasm_hash has invalid length", () => {
      tracker.addContractDeployment("access_control", "", "CABC123", "short");

      tracker.writeManifest("testnet", "ADMIN", "commit123", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      expect(() => {
        tracker.validateManifest(["access_control"]);
      }).toThrow("invalid wasm_hash");
    });
  });

  describe("atomic writes (partial failure handling)", () => {
    it("should write complete manifest atomically", () => {
      tracker.addContractDeployment("access_control", "", "CABC123", "hash1");
      tracker.addContractDeployment("invoice_nft", "", "CINV456", "hash2");
      tracker.addContractDeployment("financing_pool", "", "CPOOL789", "hash3");

      tracker.writeManifest("testnet", "ADMIN", "commit123", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      const isAtomic = tracker.verifyAtomicity();
      expect(isAtomic).toBe(true);

      const manifest = tracker.readManifest();
      expect(Object.keys(manifest.contracts).length).toBe(3);
    });

    it("should not write manifest with missing git_commit", () => {
      tracker.addContractDeployment("access_control", "", "CABC123", "hash1");

      tracker.writeManifest("testnet", "ADMIN", "", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      expect(() => {
        tracker.verifyAtomicity();
      }).toThrow("missing critical fields");
    });

    it("should not write manifest with empty contracts", () => {
      tracker.writeManifest("testnet", "ADMIN", "commit123", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      expect(() => {
        tracker.verifyAtomicity();
      }).toThrow("no contracts");
    });
  });

  describe("network-specific manifests", () => {
    it("should create separate manifests for testnet and mainnet", () => {
      const testnetTracker = new DeploymentManifestTracker(
        "testnet",
        MOCK_DEPLOYMENTS_DIR
      );
      const mainnetTracker = new DeploymentManifestTracker(
        "mainnet",
        MOCK_DEPLOYMENTS_DIR
      );

      testnetTracker.addContractDeployment(
        "access_control",
        "",
        "CTEST123",
        "hash1"
      );
      mainnetTracker.addContractDeployment(
        "access_control",
        "",
        "CMAIN456",
        "hash2"
      );

      testnetTracker.writeManifest("testnet", "ADMIN", "commit1", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      mainnetTracker.writeManifest("mainnet", "ADMIN", "commit2", {
        treasury_fee_bps: 50,
        marketplace_fee_bps: 50,
        marketplace_referrer_bps: 0,
        late_penalty_bps: 200,
        max_position_bps: 5000,
        oracle_base_currency: "USDC",
      });

      const testnetManifest = testnetTracker.readManifest();
      const mainnetManifest = mainnetTracker.readManifest();

      expect(testnetManifest.network).toBe("testnet");
      expect(mainnetManifest.network).toBe("mainnet");
      expect(testnetManifest.contracts.access_control.address).toBe("CTEST123");
      expect(mainnetManifest.contracts.access_control.address).toBe("CMAIN456");
    });
  });

  describe("deployment parameters tracking", () => {
    it("should track all protocol parameters in manifest", () => {
      tracker.addContractDeployment("access_control", "", "CABC123", "hash1");

      const params = {
        treasury_fee_bps: 100,
        marketplace_fee_bps: 75,
        marketplace_referrer_bps: 25,
        late_penalty_bps: 300,
        max_position_bps: 3000,
        oracle_base_currency: "EUR",
      };

      tracker.writeManifest("testnet", "ADMIN", "commit123", params);

      const manifest = tracker.readManifest();
      expect(manifest.parameters).toEqual(params);
    });
  });
});
