/**
 * Reporting Service — Treasury Transparency REST API Tests
 * Issue #1099
 */

import * as http from "http";
import * as os from "os";
import * as path from "path";
import * as fs from "fs";
import { createReportingServer, shapeTreasuryBalances } from "../src/httpServer";
import { ExportJobQueue } from "../src/exportJobQueue";
import { FixtureReportingDataSource } from "../src/fixtureDataSource";
import { IndexerReportingDataSource } from "../src/indexerDataSource";

function requestJson(
  server: http.Server,
  urlPath: string,
  headers: Record<string, string> = {}
): Promise<{ status: number; body: Record<string, unknown> }> {
  return new Promise((resolve, reject) => {
    const addr = server.address();
    if (!addr || typeof addr === "string") {
      return reject(new Error("Server not listening"));
    }
    const req = http.request(
      {
        hostname: "127.0.0.1",
        port: addr.port,
        path: urlPath,
        method: "GET",
        headers,
      },
      (res) => {
        let raw = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => (raw += chunk));
        res.on("end", () => {
          try {
            resolve({
              status: res.statusCode ?? 0,
              body: JSON.parse(raw) as Record<string, unknown>,
            });
          } catch (err) {
            reject(err);
          }
        });
      }
    );
    req.on("error", reject);
    req.end();
  });
}

describe("Treasury Transparency REST API (Issue #1099)", () => {
  let tmpDir: string;
  let queue: ExportJobQueue;
  let server: http.Server;

  beforeEach(async () => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "kora-treasury-test-"));
    const ds = new FixtureReportingDataSource();
    queue = new ExportJobQueue(path.join(tmpDir, "jobs.db"), ds, tmpDir);
    server = createReportingServer(queue, tmpDir, ds);
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  });

  afterEach(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    queue.close();
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  test("allows public read access without API key", async () => {
    const [balRes, infRes, outRes] = await Promise.all([
      requestJson(server, "/treasury/balances"),
      requestJson(server, "/treasury/inflows"),
      requestJson(server, "/treasury/outflows"),
    ]);

    expect(balRes.status).toBe(200);
    expect(infRes.status).toBe(200);
    expect(outRes.status).toBe(200);
  });

  test("returns per-token balances, inflows, and outflows", async () => {
    const balRes = await requestJson(server, "/treasury/balances");
    expect(balRes.status).toBe(200);
    expect(balRes.body["lastIndexedLedger"]).toBe(51200000);
    expect(balRes.body["partial"]).toBe(false);
    const balances = balRes.body["balances"] as Array<Record<string, unknown>>;
    expect(balances).toHaveLength(2);
    expect(balances.find((b) => b["asset"] === "USDC")).toMatchObject({
      asset: "USDC",
      balance: "2450000000",
      totalInflows: "3000000000",
      totalOutflows: "550000000",
    });

    const infRes = await requestJson(server, "/treasury/inflows?asset=USDC&limit=1");
    expect(infRes.status).toBe(200);
    expect(infRes.body["total"]).toBe(2);
    expect((infRes.body["inflows"] as unknown[]).length).toBe(1);

    const outRes = await requestJson(server, "/treasury/outflows");
    expect(outRes.status).toBe(200);
    expect(outRes.body["total"]).toBe(1);
    const outflows = outRes.body["outflows"] as Array<Record<string, unknown>>;
    expect(outflows[0]).toMatchObject({
      asset: "USDC",
      amount: "550000000",
      recipient: "GFIXTURE_TREASURY_OPS_RECIPIENT",
    });
  });

  test("marks partial indexer lag in response", async () => {
    const mockFetch = async (input: Parameters<typeof fetch>[0]): Promise<Response> => {
      const url = String(input);
      if (url.endsWith("/status")) {
        return new Response(
          JSON.stringify({
            lastIndexedLedger: 51199900,
            latestChainLedger: 51200050,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } }
        );
      }
      return new Response(
        JSON.stringify({
          events: [
            {
              contract: "treasury",
              event_type: "fee_collected",
              ledger_sequence: 51199900,
              event_index: 0,
              decoded_payload: {
                asset: "USDC",
                feeAmount: "100000000",
                invoiceId: "inv-lag-1",
                txHash: "0xlag1",
                timestamp: 1700000000,
              },
            },
          ],
          next_cursor: null,
        }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      );
    };

    const lagDs = new IndexerReportingDataSource({
      indexerUrl: "http://mock-indexer",
      fetchFn: mockFetch as typeof fetch,
    });
    const lagServer = createReportingServer(queue, tmpDir, lagDs);
    await new Promise<void>((resolve) =>
      lagServer.listen(0, "127.0.0.1", resolve)
    );

    try {
      const res = await requestJson(lagServer, "/treasury/balances");
      expect(res.status).toBe(200);
      expect(res.body["partial"]).toBe(true);
      expect(res.body["lastIndexedLedger"]).toBe(51199900);
      expect(res.body["latestChainLedger"]).toBe(51200050);
      expect(typeof res.body["lagWarning"]).toBe("string");
    } finally {
      await new Promise<void>((resolve) => lagServer.close(() => resolve()));
    }
  });

  test("handles empty treasury cleanly", async () => {
    const emptyFetch = async (input: Parameters<typeof fetch>[0]): Promise<Response> => {
      const url = String(input);
      if (url.endsWith("/status")) {
        return new Response(
          JSON.stringify({
            lastIndexedLedger: 51200000,
            latestChainLedger: 51200000,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } }
        );
      }
      return new Response(
        JSON.stringify({ events: [], next_cursor: null }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      );
    };

    const emptyDs = new IndexerReportingDataSource({
      indexerUrl: "http://mock-indexer",
      fetchFn: emptyFetch as typeof fetch,
    });
    const emptyServer = createReportingServer(queue, tmpDir, emptyDs);
    await new Promise<void>((resolve) =>
      emptyServer.listen(0, "127.0.0.1", resolve)
    );

    try {
      const [balRes, infRes, outRes] = await Promise.all([
        requestJson(emptyServer, "/treasury/balances"),
        requestJson(emptyServer, "/treasury/inflows"),
        requestJson(emptyServer, "/treasury/outflows"),
      ]);

      expect(balRes.status).toBe(200);
      expect(balRes.body["balances"]).toEqual([]);
      expect(balRes.body["partial"]).toBe(false);

      expect(infRes.status).toBe(200);
      expect(infRes.body["inflows"]).toEqual([]);
      expect(infRes.body["total"]).toBe(0);

      expect(outRes.status).toBe(200);
      expect(outRes.body["outflows"]).toEqual([]);
      expect(outRes.body["total"]).toBe(0);
    } finally {
      await new Promise<void>((resolve) => emptyServer.close(() => resolve()));
    }
  });

  describe("multi-asset balance view (Issue #1184)", () => {
    test("returns every asset, sorted, with an assets index", async () => {
      const res = await requestJson(server, "/treasury/balances");
      expect(res.status).toBe(200);
      expect(res.body["assets"]).toEqual(["EURC", "USDC"]);
      const balances = res.body["balances"] as Array<Record<string, unknown>>;
      expect(balances.map((b) => b["asset"])).toEqual(["EURC", "USDC"]);
      expect(balances[0]).toMatchObject({ asset: "EURC", balance: "1200000000" });
    });

    test("filters by asset case-insensitively", async () => {
      const res = await requestJson(server, "/treasury/balances?asset=eurc");
      expect(res.status).toBe(200);
      expect(res.body["assets"]).toEqual(["EURC"]);
      expect((res.body["balances"] as unknown[]).length).toBe(1);
    });

    test("unknown asset returns empty list, not an error", async () => {
      const res = await requestJson(server, "/treasury/balances?asset=XLM");
      expect(res.status).toBe(200);
      expect(res.body["balances"]).toEqual([]);
      expect(res.body["assets"]).toEqual([]);
    });

    test("rejects malformed asset codes with 400", async () => {
      for (const bad of ["", "USDC%3BDROP", "ABCDEFGHIJKLM"]) {
        const res = await requestJson(server, `/treasury/balances?asset=${bad}`);
        expect(res.status).toBe(400);
      }
    });

    test("data source failure fails closed with 500", async () => {
      const failing = new FixtureReportingDataSource();
      failing.getTreasuryBalances = async () => {
        throw new Error("indexer unavailable");
      };
      const s2 = createReportingServer(queue, tmpDir, failing);
      await new Promise<void>((resolve) => s2.listen(0, "127.0.0.1", resolve));
      try {
        const res = await requestJson(s2, "/treasury/balances");
        expect(res.status).toBe(500);
        expect(res.body["error"]).toBe("indexer unavailable");
      } finally {
        await new Promise<void>((resolve) => s2.close(() => resolve()));
      }
    });

    test("indexer source merges asset codes that differ only in case", async () => {
      const ev = (asset: string, amount: string, i: number) => ({
        contract: "treasury",
        event_type: "fee_collected",
        ledger_sequence: 100 + i,
        event_index: 0,
        decoded_payload: {
          asset,
          feeAmount: amount,
          invoiceId: `inv-${i}`,
          txHash: `0xma${i}`,
          timestamp: 1700000000 + i,
        },
      });
      const fetchFn = async (input: Parameters<typeof fetch>[0]): Promise<Response> => {
        const body = String(input).endsWith("/status")
          ? { lastIndexedLedger: 200, latestChainLedger: 200 }
          : {
              events: [ev("USDC", "100", 1), ev("usdc", "50", 2), ev("EURC", "7", 3)],
              next_cursor: null,
            };
        return new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      };
      const ds = new IndexerReportingDataSource({
        indexerUrl: "http://mock-indexer",
        fetchFn: fetchFn as typeof fetch,
      });
      const shaped = shapeTreasuryBalances(await ds.getTreasuryBalances());
      expect(shaped.assets).toEqual(["EURC", "USDC"]);
      expect(shaped.balances.find((b) => b.asset === "USDC")?.totalInflows).toBe("150");
    });
  });
});
