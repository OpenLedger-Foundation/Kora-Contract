import { MockChain } from "../src/mockChain";
import type { CompetingTxSpec } from "../src/types";

function spec(id: string, sourceIndex: number, fee: number, submitOrder: number): CompetingTxSpec {
  return { id, sourceIndex, fee, submitOrder };
}

describe("MockChain", () => {
  it("exists to keep the harness's own typing honest", () => {
    expect(new MockChain().mode).toBe("mock");
  });

  it("executes higher fees first and assigns sequential application order", async () => {
    const chain = new MockChain();
    const cheap = await chain.submit(spec("cheap", 0, 100, 0));
    const rich = await chain.submit(spec("rich", 1, 600, 1));

    await chain.advance();

    const cheapResult = await chain.observe(cheap.hash);
    const richResult = await chain.observe(rich.hash);
    expect(richResult.applicationOrder).toBe(1);
    expect(cheapResult.applicationOrder).toBe(2);
    expect(cheapResult.ledger).toBe(richResult.ledger);
  });

  it("serializes transactions from one source account across ledgers", async () => {
    const chain = new MockChain();
    const first = await chain.submit(spec("first", 0, 100, 0));
    const second = await chain.submit(spec("second", 0, 100, 1));

    await chain.advance();

    const firstResult = await chain.observe(first.hash);
    expect(firstResult.outcome).toBe("SUCCESS");
    expect((await chain.observe(second.hash)).outcome).toBe("NOT_FOUND");

    await chain.advance();

    const secondResult = await chain.observe(second.hash);
    expect(secondResult.outcome).toBe("SUCCESS");
    expect(secondResult.ledger).toBe((firstResult.ledger ?? 0) + 1);
  });

  it("honours arrival order for equal fees, and can regress it", async () => {
    const fifo = new MockChain();
    const a = await fifo.submit(spec("a", 0, 100, 0));
    const b = await fifo.submit(spec("b", 1, 100, 1));
    await fifo.advance();
    expect((await fifo.observe(a.hash)).applicationOrder).toBe(1);
    expect((await fifo.observe(b.hash)).applicationOrder).toBe(2);

    const lifo = new MockChain({ equalFeeTieBreak: "reverse" });
    const c = await lifo.submit(spec("c", 0, 100, 0));
    const d = await lifo.submit(spec("d", 1, 100, 1));
    await lifo.advance();
    expect((await lifo.observe(c.hash)).applicationOrder).toBe(2);
    expect((await lifo.observe(d.hash)).applicationOrder).toBe(1);
  });

  it("can ignore fee priority to simulate a regression", async () => {
    const chain = new MockChain({ disableFeePriority: true });
    const rich = await chain.submit(spec("rich", 0, 600, 0));
    const cheap = await chain.submit(spec("cheap", 1, 100, 1));
    await chain.advance();
    expect((await chain.observe(rich.hash)).applicationOrder).toBe(1);
    expect((await chain.observe(cheap.hash)).applicationOrder).toBe(2);
  });

  it("can reject submissions to exercise partial failure handling", async () => {
    const chain = new MockChain({ submitFailureRate: 1, random: () => 0 });
    await expect(chain.submit(spec("x", 0, 100, 0))).rejects.toThrow(/submission rejected/);
  });

  it("withholds confirmations while simulating RPC lag", async () => {
    const chain = new MockChain({ rpcLagLedgers: 1 });
    const tx = await chain.submit(spec("lagged", 0, 100, 0));
    await chain.advance();
    expect((await chain.observe(tx.hash)).outcome).toBe("NOT_FOUND");

    await chain.advance();
    expect((await chain.observe(tx.hash)).outcome).toBe("SUCCESS");
  });
});
