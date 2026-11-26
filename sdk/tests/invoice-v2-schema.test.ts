/**
 * SDK Invoice type — schema v2 parity tests
 *
 * Verifies that `decodeInvoice` correctly maps every field defined in the
 * on-chain `kora_shared::types::Invoice` struct (Rust schema v2) to the
 * TypeScript `Invoice` interface, and that both pre-migration (v1) and
 * post-migration (v2) records are decoded without data loss.
 *
 * Test IDs reference rows in the acceptance criteria:
 *   INV-V2-1  All v2 fields present in the TypeScript interface
 *   INV-V2-2  decodeInvoice populates v2 fields from a v2 ScVal fixture
 *   INV-V2-3  decodeInvoice falls back gracefully on a v1 (pre-migration) record
 *   INV-V2-4  Unknown / extra fields in the ScVal map are silently ignored
 *   INV-V2-5  Optional fields (fundedAt, repaidAt, notes) accept null
 *   INV-V2-6  metadataHash: empty Buffer on un-committed records (length != 32)
 *   INV-V2-7  metadataHash: 32-byte Buffer on committed records
 *   INV-V2-8  RiskTier enum exhaustive round-trip
 *   INV-V2-9  InvoiceStatus enum exhaustive round-trip
 */

import { xdr, nativeToScVal, Keypair } from "@stellar/stellar-sdk";
import { Invoice, InvoiceStatus, RiskTier, decodeInvoice } from "../src/types";

// ── Helpers ───────────────────────────────────────────────────────────────────

/**
 * Build a minimal `ScVal` map that matches the on-chain Invoice encoding
 * produced by Soroban's `contracttype` derive macro.
 *
 * Keys are `ScvSym`; values follow the same encoding used in the on-chain
 * contract and by the `decodeInvoice` decoder.
 */
function buildInvoiceScVal(fields: {
  id?: bigint;
  sme?: string;
  debtor_hash?: Buffer;
  amount?: bigint;
  currency?: string;
  due_date?: bigint;
  ipfs_cid?: string;
  metadata_hash?: Buffer;
  risk_score?: number;
  risk_tier?: string;
  status?: string;
  created_at?: bigint;
  funded_at?: bigint | null;
  repaid_at?: bigint | null;
  notes?: string | null;
  // extra keys to test unknown-field handling
  [extra: string]: unknown;
}): xdr.ScVal {
  const {
    id = 1n,
    sme = Keypair.random().publicKey(),
    debtor_hash = Buffer.alloc(32, 0xab),
    amount = 10_000_000_000n,
    currency = "USDC",
    due_date = 1_800_000_000n,
    ipfs_cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
    metadata_hash = Buffer.alloc(32, 0x11),
    risk_score = 30,
    risk_tier = "AA",
    status = "Created",
    created_at = 1_700_000_000n,
    funded_at = null,
    repaid_at = null,
    notes = null,
    ...extras
  } = fields;

  const entries: xdr.ScMapEntry[] = [];

  function kv(key: string, val: xdr.ScVal): xdr.ScMapEntry {
    return new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol(key),
      val,
    });
  }

  // Encode u64 as ScvU64
  function encU64(v: bigint): xdr.ScVal {
    return xdr.ScVal.scvU64(new xdr.Uint64(Number(v)));
  }

  // Encode Option<u64> as ScvVec([]) for None, ScvVec([u64]) for Some
  function encOptU64(v: bigint | null): xdr.ScVal {
    if (v === null) return xdr.ScVal.scvVec([]);
    return xdr.ScVal.scvVec([encU64(v)]);
  }

  // Encode Option<String> as ScvVec([]) for None, ScvVec([str]) for Some
  function encOptStr(v: string | null): xdr.ScVal {
    if (v === null) return xdr.ScVal.scvVec([]);
    return xdr.ScVal.scvVec([xdr.ScVal.scvString(v)]);
  }

  // Encode enum variant as ScvVec([sym])
  function encEnum(variant: string): xdr.ScVal {
    return xdr.ScVal.scvVec([xdr.ScVal.scvSymbol(variant)]);
  }

  // Encode i128
  function encI128(v: bigint): xdr.ScVal {
    const lo = BigInt.asUintN(64, v);
    const hi = BigInt.asIntN(64, v >> 64n);
    return xdr.ScVal.scvI128(
      new xdr.Int128Parts({
        lo: xdr.Uint64.fromXDR(
          Buffer.from(lo.toString(16).padStart(16, "0"), "hex")
        ),
        hi: xdr.Int64.fromXDR(
          Buffer.from(
            ((hi < 0n ? hi + (1n << 64n) : hi)).toString(16).padStart(16, "0"),
            "hex"
          )
        ),
      })
    );
  }

  // Encode Address as ScvAddress (account)
  function encAddr(pub: string): xdr.ScVal {
    return xdr.ScVal.scvAddress(
      xdr.ScAddress.scAddressTypeAccount(
        xdr.PublicKey.publicKeyTypeEd25519(
          Buffer.from(pub.length === 56 ? pub : Keypair.random().publicKey(), "hex")
        )
      )
    );
  }

  entries.push(kv("id", encU64(id)));
  // Encode sme as hex bytes (the decoder uses v.address().accountId().ed25519())
  // For test purposes encode as raw bytes and use a simpler address path.
  // We use the reqAddr helper which calls v.address(), so we need a real ScvAddress.
  // Use a fresh random keypair address encoded properly.
  const smePub = Keypair.random();
  entries.push(
    new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol("sme"),
      val: xdr.ScVal.scvAddress(
        xdr.ScAddress.scAddressTypeAccount(
          xdr.PublicKey.publicKeyTypeEd25519(smePub.rawPublicKey())
        )
      ),
    })
  );
  entries.push(kv("debtor_hash", xdr.ScVal.scvBytes(debtor_hash)));
  entries.push(kv("amount", encI128(amount)));
  entries.push(kv("currency", xdr.ScVal.scvSymbol(currency)));
  entries.push(kv("due_date", encU64(due_date)));
  entries.push(kv("ipfs_cid", xdr.ScVal.scvString(ipfs_cid)));
  entries.push(kv("metadata_hash", xdr.ScVal.scvBytes(metadata_hash)));
  entries.push(kv("risk_score", xdr.ScVal.scvU32(risk_score)));
  entries.push(kv("risk_tier", encEnum(risk_tier)));
  entries.push(kv("status", encEnum(status)));
  entries.push(kv("created_at", encU64(created_at)));
  entries.push(kv("funded_at", encOptU64(funded_at)));
  entries.push(kv("repaid_at", encOptU64(repaid_at)));
  entries.push(kv("notes", encOptStr(notes)));

  // Inject any extra keys (unknown-field tests)
  for (const [k, v] of Object.entries(extras)) {
    entries.push(kv(k, xdr.ScVal.scvString(String(v))));
  }

  return xdr.ScVal.scvMap(entries);
}

// ── INV-V2-1: Interface completeness ─────────────────────────────────────────

describe("INV-V2-1: Invoice interface completeness", () => {
  it("TypeScript Invoice interface declares all schema v2 fields", () => {
    // Compile-time check: construct a typed object with every field.
    // If a field is missing from the interface this will be a TypeScript error.
    const v2: Invoice = {
      id: 1n,
      sme: "GABC",
      debtorHash: Buffer.alloc(32),
      amount: 10_000_000_000n,
      currency: "USDC",
      dueDate: 1_800_000_000n,
      ipfsCid: "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
      // ── v2 fields ──
      metadataHash: Buffer.alloc(32, 0x11),
      riskScore: 30,
      riskTier: "AA",
      status: "Created",
      createdAt: 1_700_000_000n,
      fundedAt: null,
      repaidAt: null,
      notes: null,
    };

    expect(v2).toHaveProperty("metadataHash");
    expect(v2).toHaveProperty("notes");
    expect(Buffer.isBuffer(v2.metadataHash)).toBe(true);
  });

  it("Invoice interface requires exactly the v2 field set (no extra required fields)", () => {
    // A v2 record with both optional-nullable fields set
    const full: Invoice = {
      id: 42n,
      sme: "GABC",
      debtorHash: Buffer.alloc(32, 0xde),
      amount: 5_000_000_000n,
      currency: "EURC",
      dueDate: 1_900_000_000n,
      ipfsCid: "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG",
      metadataHash: Buffer.alloc(32, 0xcc),
      riskScore: 55,
      riskTier: "A",
      status: "Funded",
      createdAt: 1_700_000_000n,
      fundedAt: 1_750_000_000n,
      repaidAt: null,
      notes: "first batch, net-60",
    };
    expect(full.notes).toBe("first batch, net-60");
    expect(full.fundedAt).toBe(1_750_000_000n);
  });
});

// ── INV-V2-2: decodeInvoice — v2 record ───────────────────────────────────────

describe("INV-V2-2: decodeInvoice populates all v2 fields", () => {
  const METADATA = Buffer.alloc(32, 0x42);
  const DEBTOR   = Buffer.alloc(32, 0xab);

  let decoded: Invoice;

  beforeAll(() => {
    const scval = buildInvoiceScVal({
      id: 7n,
      debtor_hash: DEBTOR,
      amount: 10_000_000_000n,
      currency: "USDC",
      due_date: 1_800_000_000n,
      ipfs_cid: "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
      metadata_hash: METADATA,
      risk_score: 25,
      risk_tier: "AA",
      status: "Listed",
      created_at: 1_700_000_000n,
      funded_at: null,
      repaid_at: null,
      notes: "test memo",
    });
    decoded = decodeInvoice(scval);
  });

  it("id is decoded correctly", () => {
    expect(decoded.id).toBe(7n);
  });

  it("amount is decoded as bigint via i128 path", () => {
    expect(decoded.amount).toBe(10_000_000_000n);
  });

  it("currency is decoded as string from symbol", () => {
    expect(decoded.currency).toBe("USDC");
  });

  it("dueDate is decoded as bigint", () => {
    expect(decoded.dueDate).toBe(1_800_000_000n);
  });

  it("ipfsCid is decoded as string", () => {
    expect(decoded.ipfsCid).toBe(
      "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
    );
  });

  it("[v2] metadataHash is a 32-byte Buffer", () => {
    expect(Buffer.isBuffer(decoded.metadataHash)).toBe(true);
    expect(decoded.metadataHash.length).toBe(32);
    expect(decoded.metadataHash.equals(METADATA)).toBe(true);
  });

  it("[v2] notes is decoded as a string when present", () => {
    expect(decoded.notes).toBe("test memo");
  });

  it("riskScore is decoded as number", () => {
    expect(decoded.riskScore).toBe(25);
  });

  it("riskTier is decoded as string enum", () => {
    expect(decoded.riskTier).toBe("AA");
  });

  it("status is decoded as string enum", () => {
    expect(decoded.status).toBe("Listed");
  });

  it("fundedAt is null when Option<u64> is None", () => {
    expect(decoded.fundedAt).toBeNull();
  });

  it("repaidAt is null when Option<u64> is None", () => {
    expect(decoded.repaidAt).toBeNull();
  });
});

// ── INV-V2-3: decodeInvoice — v1 (pre-migration) record ──────────────────────

describe("INV-V2-3: decodeInvoice handles v1 (pre-migration) records", () => {
  it("missing metadata_hash field returns empty Buffer (length 0)", () => {
    // Build a ScVal map that intentionally omits metadata_hash (v1 record)
    const entries: xdr.ScMapEntry[] = [];
    const kv = (k: string, v: xdr.ScVal) =>
      new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol(k), val: v });

    const smePub = Keypair.random();
    entries.push(kv("id", xdr.ScVal.scvU64(new xdr.Uint64(1))));
    entries.push(
      new xdr.ScMapEntry({
        key: xdr.ScVal.scvSymbol("sme"),
        val: xdr.ScVal.scvAddress(
          xdr.ScAddress.scAddressTypeAccount(
            xdr.PublicKey.publicKeyTypeEd25519(smePub.rawPublicKey())
          )
        ),
      })
    );
    entries.push(kv("debtor_hash", xdr.ScVal.scvBytes(Buffer.alloc(32))));
    entries.push(
      kv(
        "amount",
        xdr.ScVal.scvI128(
          new xdr.Int128Parts({
            lo: xdr.Uint64.fromXDR(Buffer.alloc(8)),
            hi: xdr.Int64.fromXDR(Buffer.alloc(8)),
          })
        )
      )
    );
    entries.push(kv("currency", xdr.ScVal.scvSymbol("USDC")));
    entries.push(kv("due_date", xdr.ScVal.scvU64(new xdr.Uint64(1_800_000_000))));
    entries.push(kv("ipfs_cid", xdr.ScVal.scvString("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG")));
    // metadata_hash intentionally omitted
    entries.push(kv("risk_score", xdr.ScVal.scvU32(50)));
    entries.push(kv("risk_tier", xdr.ScVal.scvVec([xdr.ScVal.scvSymbol("A")])));
    entries.push(kv("status", xdr.ScVal.scvVec([xdr.ScVal.scvSymbol("Created")])));
    entries.push(kv("created_at", xdr.ScVal.scvU64(new xdr.Uint64(1_700_000_000))));
    entries.push(kv("funded_at", xdr.ScVal.scvVec([])));
    entries.push(kv("repaid_at", xdr.ScVal.scvVec([])));
    // notes intentionally omitted (v1 record)

    const scval = xdr.ScVal.scvMap(entries);
    const decoded = decodeInvoice(scval);

    // metadataHash falls back to empty Buffer — dApps must check length before treating as committed
    expect(Buffer.isBuffer(decoded.metadataHash)).toBe(true);
    expect(decoded.metadataHash.length).toBe(0);
    expect(decoded.metadataHash.length).not.toBe(32);
  });

  it("missing notes field returns null", () => {
    const entries: xdr.ScMapEntry[] = [];
    const kv = (k: string, v: xdr.ScVal) =>
      new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol(k), val: v });

    const smePub = Keypair.random();
    entries.push(kv("id", xdr.ScVal.scvU64(new xdr.Uint64(2))));
    entries.push(
      new xdr.ScMapEntry({
        key: xdr.ScVal.scvSymbol("sme"),
        val: xdr.ScVal.scvAddress(
          xdr.ScAddress.scAddressTypeAccount(
            xdr.PublicKey.publicKeyTypeEd25519(smePub.rawPublicKey())
          )
        ),
      })
    );
    entries.push(kv("debtor_hash", xdr.ScVal.scvBytes(Buffer.alloc(32))));
    entries.push(
      kv(
        "amount",
        xdr.ScVal.scvI128(
          new xdr.Int128Parts({
            lo: xdr.Uint64.fromXDR(Buffer.alloc(8)),
            hi: xdr.Int64.fromXDR(Buffer.alloc(8)),
          })
        )
      )
    );
    entries.push(kv("currency", xdr.ScVal.scvSymbol("USDC")));
    entries.push(kv("due_date", xdr.ScVal.scvU64(new xdr.Uint64(1_800_000_000))));
    entries.push(kv("ipfs_cid", xdr.ScVal.scvString("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG")));
    entries.push(kv("metadata_hash", xdr.ScVal.scvBytes(Buffer.alloc(0))));
    entries.push(kv("risk_score", xdr.ScVal.scvU32(50)));
    entries.push(kv("risk_tier", xdr.ScVal.scvVec([xdr.ScVal.scvSymbol("A")])));
    entries.push(kv("status", xdr.ScVal.scvVec([xdr.ScVal.scvSymbol("Created")])));
    entries.push(kv("created_at", xdr.ScVal.scvU64(new xdr.Uint64(1_700_000_000))));
    entries.push(kv("funded_at", xdr.ScVal.scvVec([])));
    entries.push(kv("repaid_at", xdr.ScVal.scvVec([])));
    // notes key absent

    const scval = xdr.ScVal.scvMap(entries);
    const decoded = decodeInvoice(scval);

    // notes must be null, not throw
    expect(decoded.notes).toBeNull();
  });
});

// ── INV-V2-4: Unknown field policy ───────────────────────────────────────────

describe("INV-V2-4: Unknown / extra fields are silently ignored", () => {
  it("extra ScVal map keys do not cause decodeInvoice to throw", () => {
    const scval = buildInvoiceScVal({
      notes: null,
      metadata_hash: Buffer.alloc(32, 0x55),
      // Inject future / unknown fields
      future_field_v3: "some future value",
      another_unknown_key: "12345",
    });

    // Must not throw; unknown keys are silently ignored
    expect(() => decodeInvoice(scval)).not.toThrow();
    const decoded = decodeInvoice(scval);
    expect(decoded.id).toBe(1n);
    expect(decoded.metadataHash.length).toBe(32);
  });

  it("extra keys do not appear on the decoded Invoice object", () => {
    const scval = buildInvoiceScVal({
      future_field_v3: "extra",
    });
    const decoded = decodeInvoice(scval) as unknown as Record<string, unknown>;
    expect(decoded["future_field_v3"]).toBeUndefined();
    expect(decoded["another_unknown_key"]).toBeUndefined();
  });
});

// ── INV-V2-5: Optional fields accept null ─────────────────────────────────────

describe("INV-V2-5: Optional fields accept null", () => {
  it("fundedAt can be null", () => {
    const scval = buildInvoiceScVal({ funded_at: null });
    expect(decodeInvoice(scval).fundedAt).toBeNull();
  });

  it("repaidAt can be null", () => {
    const scval = buildInvoiceScVal({ repaid_at: null });
    expect(decodeInvoice(scval).repaidAt).toBeNull();
  });

  it("notes can be null", () => {
    const scval = buildInvoiceScVal({ notes: null });
    expect(decodeInvoice(scval).notes).toBeNull();
  });

  it("fundedAt can be a bigint when Some", () => {
    const scval = buildInvoiceScVal({ funded_at: 1_750_000_000n, status: "Funded" });
    expect(decodeInvoice(scval).fundedAt).toBe(1_750_000_000n);
  });

  it("repaidAt can be a bigint when Some", () => {
    const scval = buildInvoiceScVal({
      funded_at: 1_750_000_000n,
      repaid_at: 1_780_000_000n,
      status: "Repaid",
    });
    expect(decodeInvoice(scval).repaidAt).toBe(1_780_000_000n);
  });

  it("notes can be a non-empty string when Some", () => {
    const scval = buildInvoiceScVal({ notes: "net-60 invoice batch A" });
    expect(decodeInvoice(scval).notes).toBe("net-60 invoice batch A");
  });
});

// ── INV-V2-6/7: metadataHash length semantics ────────────────────────────────

describe("INV-V2-6/7: metadataHash length communicates commitment state", () => {
  it("[INV-V2-6] empty metadata_hash (length 0) means un-committed", () => {
    const scval = buildInvoiceScVal({ metadata_hash: Buffer.alloc(0) });
    const decoded = decodeInvoice(scval);
    expect(decoded.metadataHash.length).toBe(0);
    // dApp guard: check length before treating as a committed hash
    expect(decoded.metadataHash.length === 32).toBe(false);
  });

  it("[INV-V2-7] 32-byte metadata_hash means committed", () => {
    const hash = Buffer.from("a".repeat(64), "hex"); // 32 bytes
    const scval = buildInvoiceScVal({ metadata_hash: hash });
    const decoded = decodeInvoice(scval);
    expect(decoded.metadataHash.length).toBe(32);
    expect(decoded.metadataHash.equals(hash)).toBe(true);
  });

  it("[INV-V2-6] partial hash (< 32 bytes) is not treated as committed", () => {
    const scval = buildInvoiceScVal({ metadata_hash: Buffer.alloc(16, 0xab) });
    const decoded = decodeInvoice(scval);
    expect(decoded.metadataHash.length).not.toBe(32);
  });
});

// ── INV-V2-8: RiskTier enum exhaustive round-trip ─────────────────────────────

describe("INV-V2-8: RiskTier enum exhaustive round-trip", () => {
  const tiers: RiskTier[] = ["AAA", "AA", "A", "B", "C"];

  for (const tier of tiers) {
    it(`RiskTier "${tier}" survives encode → decode`, () => {
      const scval = buildInvoiceScVal({ risk_tier: tier });
      const decoded = decodeInvoice(scval);
      expect(decoded.riskTier).toBe(tier);
    });
  }
});

// ── INV-V2-9: InvoiceStatus enum exhaustive round-trip ───────────────────────

describe("INV-V2-9: InvoiceStatus enum exhaustive round-trip", () => {
  const statuses: InvoiceStatus[] = [
    "Created",
    "Listed",
    "Funded",
    "Repaid",
    "Defaulted",
  ];

  for (const status of statuses) {
    it(`InvoiceStatus "${status}" survives encode → decode`, () => {
      const scval = buildInvoiceScVal({ status });
      const decoded = decodeInvoice(scval);
      expect(decoded.status).toBe(status);
    });
  }
});

// ── decodeInvoice error handling ──────────────────────────────────────────────

describe("decodeInvoice error handling", () => {
  it("throws when given a non-ScvMap value", () => {
    expect(() => decodeInvoice(xdr.ScVal.scvVoid())).toThrow(
      /expected ScvMap/i
    );
  });

  it("throws when a required field is missing", () => {
    // Build a map without the required 'ipfs_cid' field
    const entries: xdr.ScMapEntry[] = [
      new xdr.ScMapEntry({
        key: xdr.ScVal.scvSymbol("id"),
        val: xdr.ScVal.scvU64(new xdr.Uint64(1)),
      }),
    ];
    const incomplete = xdr.ScVal.scvMap(entries);
    expect(() => decodeInvoice(incomplete)).toThrow();
  });
});
