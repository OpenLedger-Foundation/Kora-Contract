import {
  contentMatchesType,
  DEFAULT_PINNING_CONFIG,
  normalizeContentType,
  sha256Hex,
  PinningService,
} from "../src/service";
import {
  HttpPinProvider,
  InMemoryPinProvider,
  PinProvider,
  PinProviderError,
} from "../src/providers";
import { PinStore } from "../src/store";
import { PinVerifier } from "../src/verifier";

function req(content: string, overrides: Record<string, unknown> = {}) {
  const bytes = new TextEncoder().encode(content);
  return {
    filename: "invoice.json",
    content: bytes,
    contentType: "application/json",
    claimedSha256Hex: sha256Hex(bytes),
    piiHandled: true,
    ...overrides,
  };
}

describe("ipfs-pinning (issue #754)", () => {
  test("dual-provider pin success returns a CID", async () => {
    const store = new PinStore();
    const svc = new PinningService(
      [new InMemoryPinProvider("a"), new InMemoryPinProvider("b")],
      store,
    );
    const res = await svc.upload(req('{"invoice":1}'));
    expect(res.cid).toHaveLength(64);
    expect(res.degraded).toBe(false);
    expect(res.providers.filter((p) => p.ok)).toHaveLength(2);
    expect(store.get(res.cid)?.status).toBe("pinned");
    store.close();
  });

  test("partial failure degrades instead of failing when minSuccesses=1", async () => {
    const store = new PinStore();
    const svc = new PinningService(
      [new InMemoryPinProvider("a"), new InMemoryPinProvider("b", { failPin: true })],
      store,
      { ...DEFAULT_PINNING_CONFIG, pinRetryAttempts: 1 },
    );
    const res = await svc.upload(req('{"invoice":2}'));
    expect(res.degraded).toBe(true);
    expect(res.providers.filter((p) => p.ok)).toHaveLength(1);
    expect(store.get(res.cid)?.status).toBe("degraded");
    store.close();
  });

  test("total failure throws and records failed status", async () => {
    const store = new PinStore();
    const svc = new PinningService(
      [
        new InMemoryPinProvider("a", { failPin: true }),
        new InMemoryPinProvider("b", { failPin: true }),
      ],
      store,
      { ...DEFAULT_PINNING_CONFIG, pinRetryAttempts: 1 },
    );
    await expect(svc.upload(req('{"invoice":3}'))).rejects.toThrow(/PIN_FAILED/);
    store.close();
  });

  test("retries transient provider failures with capped exponential backoff", async () => {
    const store = new PinStore();
    const transient = {
      id: "transient",
      pin: jest.fn()
        .mockRejectedValueOnce(new Error("network timeout"))
        .mockRejectedValueOnce(new Error("network timeout"))
        .mockResolvedValue(undefined),
      isPinned: jest.fn().mockResolvedValue(true),
      repin: jest.fn().mockResolvedValue(undefined),
    } satisfies PinProvider;
    const delays: number[] = [];
    const svc = new PinningService(
      [transient, new InMemoryPinProvider("healthy")],
      store,
      {
        ...DEFAULT_PINNING_CONFIG,
        pinRetryAttempts: 3,
        pinRetryBackoffMs: 100,
        pinRetryMaxBackoffMs: 150,
      },
      async (ms) => { delays.push(ms); },
    );

    await svc.upload(req('{"invoice":"retry"}'));

    expect(transient.pin).toHaveBeenCalledTimes(3);
    expect(delays).toEqual([100, 150]);
    expect(svc.getMetrics()).toMatchObject({ pinAttempts: 4, pinRetries: 2 });
    store.close();
  });

  test("stores permanent provider failures in the DLQ and supports verified replay", async () => {
    const store = new PinStore();
    const permanent = {
      id: "permanent",
      pin: jest.fn()
        .mockRejectedValueOnce(new PinProviderError("HTTP 400", false, 400))
        .mockResolvedValue(undefined),
      isPinned: jest.fn().mockResolvedValue(true),
      repin: jest.fn().mockResolvedValue(undefined),
    } satisfies PinProvider;
    const svc = new PinningService(
      [permanent, new InMemoryPinProvider("healthy")],
      store,
      { ...DEFAULT_PINNING_CONFIG, pinRetryAttempts: 3 },
      async () => undefined,
    );
    const upload = req('{"invoice":"dlq"}');
    const result = await svc.upload(upload);
    const [deadLetter] = store.listDeadLetters();

    expect(result.degraded).toBe(true);
    expect(deadLetter).toMatchObject({
      cid: result.cid,
      providerId: "permanent",
      status: "pending",
      attempts: 0,
    });
    await expect(svc.replayDeadLetter(deadLetter.id, new TextEncoder().encode("wrong")))
      .rejects.toThrow("INTEGRITY_MISMATCH");
    const replayed = await svc.replayDeadLetter(deadLetter.id, upload.content);

    expect(replayed).toMatchObject({ id: deadLetter.id, status: "resolved", attempts: 1 });
    expect(svc.getMetrics()).toMatchObject({
      permanentFailures: 1,
      deadLetterEntries: 1,
      deadLetterReplayAttempts: 1,
      deadLetterReplaySuccesses: 1,
      pendingDeadLetters: 0,
    });
    expect(svc.metricsText()).toContain("kora_ipfs_pin_dead_letter_pending 0");
    store.close();
  });

  test("classifies HTTP 4xx responses as permanent and 5xx responses as retryable", async () => {
    const permanent = new HttpPinProvider(
      "permanent",
      "https://pins.example",
      "token",
      jest.fn().mockResolvedValue({ ok: false, status: 400 } as Response),
    );
    const transient = new HttpPinProvider(
      "transient",
      "https://pins.example",
      "token",
      jest.fn().mockResolvedValue({ ok: false, status: 503 } as Response),
    );
    await expect(permanent.pin("cid", new Uint8Array()))
      .rejects.toMatchObject({ retryable: false, statusCode: 400 });
    await expect(transient.pin("cid", new Uint8Array()))
      .rejects.toMatchObject({ retryable: true, statusCode: 503 });
  });

  test("integrity-hash mismatch is rejected before pinning", async () => {
    const store = new PinStore();
    const svc = new PinningService(
      [new InMemoryPinProvider("a"), new InMemoryPinProvider("b")],
      store,
    );
    await expect(
      svc.upload(req("hello", { claimedSha256Hex: "00".repeat(32) })),
    ).rejects.toThrow(/INTEGRITY_MISMATCH/);
    expect(store.listAll()).toHaveLength(0);
    store.close();
  });

  test("size cap, type allowlist, and PII gate are enforced", async () => {
    const store = new PinStore();
    const svc = new PinningService(
      [new InMemoryPinProvider("a"), new InMemoryPinProvider("b")],
      store,
      {
        maxFileSizeBytes: 4,
        allowedContentTypes: ["application/json"],
        allowedExtensions: [".json"],
      },
    );
    await expect(svc.upload(req("12345"))).rejects.toThrow(/FILE_TOO_LARGE/);
    await expect(
      svc.upload(req("a", { contentType: "video/mp4", filename: "x.mp4" })),
    ).rejects.toThrow(/UNSUPPORTED_TYPE/);
    await expect(svc.upload(req("a", { piiHandled: false }))).rejects.toThrow(/PII_NOT_HANDLED/);
    store.close();
  });

  test("periodic verifier re-pins on failure and alerts past threshold", async () => {
    const store = new PinStore();
    const a = new InMemoryPinProvider("a");
    const b = new InMemoryPinProvider("b");
    const svc = new PinningService([a, b], store);
    const alerts: string[] = [];
    const verifier = new PinVerifier([a, b], store, svc, {
      intervalMs: 1_000_000,
      alertAfterFailures: 1,
      onAlert: (_rec, msg) => alerts.push(msg),
    });
    const res = await svc.upload(req('{"invoice":9}'));
    // Simulate provider-side loss on b where cache still holds content → repin.
    b.drop(res.cid);
    const out = await verifier.verifyOnce();
    expect(out.repinned).toContain(res.cid);

    // Simulate loss with NO cached content (fresh service instance sharing the store).
    const svc2 = new PinningService([a, b], store);
    const verifier2 = new PinVerifier([a, b], store, svc2, {
      intervalMs: 1_000_000,
      alertAfterFailures: 1,
      onAlert: (_rec, msg) => alerts.push(msg),
    });
    b.drop(res.cid);
    const out2 = await verifier2.verifyOnce();
    expect(out2.failed).toContain(res.cid);
    expect(alerts.length).toBeGreaterThan(0);
    store.close();
  });
});

describe("content-type allowlist (issue #1186)", () => {
  const providers = () => [new InMemoryPinProvider("a"), new InMemoryPinProvider("b")];
  const bytesReq = (bytes: Uint8Array, filename: string, contentType: string) => ({
    filename,
    content: bytes,
    contentType,
    claimedSha256Hex: sha256Hex(bytes),
    piiHandled: true,
  });
  const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0]);
  const JPEG = new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0]);
  const PDF = new TextEncoder().encode("%PDF-1.7\n...");

  test("accepts allowlisted types whose content matches", async () => {
    const store = new PinStore();
    const svc = new PinningService(providers(), store);
    await expect(svc.upload(bytesReq(PNG, "scan.png", "image/png"))).resolves.toHaveProperty("cid");
    await expect(svc.upload(bytesReq(JPEG, "scan.JPEG", "image/jpeg"))).resolves.toHaveProperty("cid");
    await expect(svc.upload(bytesReq(PDF, "invoice.pdf", "application/pdf"))).resolves.toHaveProperty("cid");
    store.close();
  });

  test("normalizes case and utf-8 charset parameter", async () => {
    const store = new PinStore();
    const svc = new PinningService(providers(), store);
    await expect(
      svc.upload(req('{"invoice":9}', { contentType: " Application/JSON; charset=\"UTF-8\" " })),
    ).resolves.toHaveProperty("cid");
    store.close();
  });

  test("rejects unknown MIME parameters and missing type", async () => {
    const store = new PinStore();
    const svc = new PinningService(providers(), store);
    await expect(
      svc.upload(req('{"a":1}', { contentType: "application/json; charset=latin1" })),
    ).rejects.toThrow(/UNSUPPORTED_TYPE/);
    await expect(
      svc.upload(req('{"a":1}', { contentType: "application/json; boundary=x" })),
    ).rejects.toThrow(/UNSUPPORTED_TYPE/);
    await expect(svc.upload(req('{"a":1}', { contentType: "" }))).rejects.toThrow(/UNSUPPORTED_TYPE/);
    await expect(svc.upload(req("<svg/>", { contentType: "image/svg+xml", filename: "x.svg" }))).rejects.toThrow(
      /UNSUPPORTED_TYPE/,
    );
    store.close();
  });

  test("rejects extension that does not belong to the declared type", async () => {
    const store = new PinStore();
    const svc = new PinningService(providers(), store);
    await expect(svc.upload(bytesReq(PNG, "scan.pdf", "image/png"))).rejects.toThrow(
      /EXTENSION_TYPE_MISMATCH/,
    );
    store.close();
  });

  test("rejects disguised HTML / non-JSON / invalid UTF-8 content", async () => {
    const store = new PinStore();
    const svc = new PinningService(providers(), store);
    const html = new TextEncoder().encode("<html><script>alert(1)</script></html>");
    await expect(svc.upload(bytesReq(html, "scan.png", "image/png"))).rejects.toThrow(/CONTENT_TYPE_MISMATCH/);
    await expect(svc.upload(bytesReq(html, "scan.jpg", "image/jpeg"))).rejects.toThrow(/CONTENT_TYPE_MISMATCH/);
    await expect(svc.upload(bytesReq(html, "i.pdf", "application/pdf"))).rejects.toThrow(/CONTENT_TYPE_MISMATCH/);
    await expect(svc.upload(bytesReq(html, "i.json", "application/json"))).rejects.toThrow(/CONTENT_TYPE_MISMATCH/);
    const badUtf8 = new Uint8Array([0x22, 0xff, 0x22]);
    await expect(svc.upload(bytesReq(badUtf8, "i.json", "application/json"))).rejects.toThrow(/CONTENT_TYPE_MISMATCH/);
    expect(store.listDeadLetters()).toHaveLength(0);
    store.close();
  });

  test("enforces per-content-type size caps", async () => {
    const store = new PinStore();
    const svc = new PinningService(providers(), store, {
      ...DEFAULT_PINNING_CONFIG,
      maxSizeBytesByContentType: { "application/json": 8 },
    });
    await expect(svc.upload(req('{"invoice":123}'))).rejects.toThrow(/FILE_TOO_LARGE: 15 > 8/);
    await expect(svc.upload(bytesReq(PNG, "scan.png", "image/png"))).resolves.toHaveProperty("cid");
    store.close();
  });

  test("fails closed on empty or invalid allowlist configuration", () => {
    const store = new PinStore();
    const base = DEFAULT_PINNING_CONFIG;
    expect(() => new PinningService(providers(), store, { ...base, allowedContentTypes: [] })).toThrow(/CONFIG_ERROR/);
    expect(() => new PinningService(providers(), store, { ...base, allowedExtensions: [] })).toThrow(/CONFIG_ERROR/);
    expect(() => new PinningService(providers(), store, { ...base, maxFileSizeBytes: 0 })).toThrow(/CONFIG_ERROR/);
    expect(
      () => new PinningService(providers(), store, { ...base, allowedContentTypes: ["text/html"] }),
    ).toThrow(/CONFIG_ERROR/);
    expect(
      () =>
        new PinningService(providers(), store, {
          ...base,
          maxSizeBytesByContentType: { "application/json": -1 },
        }),
    ).toThrow(/CONFIG_ERROR/);
    store.close();
  });

  test("unknown types never pass content sniffing", () => {
    expect(contentMatchesType("text/html", new Uint8Array([1]))).toBe(false);
    expect(normalizeContentType("IMAGE/PNG;")).toBe("image/png");
  });
});
