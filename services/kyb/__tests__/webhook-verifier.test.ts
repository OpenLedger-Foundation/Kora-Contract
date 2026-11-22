import { createHmac } from "node:crypto";
import {
  InMemoryWebhookReplayCache,
  verifyKybWebhook,
} from "../webhook-verifier";

const vector = require("./fixtures/webhook-vector.json");
const nowSeconds = Number(vector.timestamp) + 60;

describe("KYB webhook verification", () => {
  it("accepts the provider signature vector once and rejects replay", () => {
    const replayCache = new InMemoryWebhookReplayCache();
    const input = {
      timestamp: vector.timestamp,
      signature: `sha256=${vector.signature}`,
      rawBody: Buffer.from(vector.rawBody),
    };
    const config = { secret: vector.secret, replayCache, nowSeconds: () => nowSeconds };

    expect(() => verifyKybWebhook(input, config)).not.toThrow();
    expect(() => verifyKybWebhook(input, config)).toThrow("replay detected");
  });

  it("rejects stale timestamps, malformed signatures, and body tampering", () => {
    const makeConfig = () => ({
      secret: vector.secret,
      replayCache: new InMemoryWebhookReplayCache(),
      nowSeconds: () => nowSeconds,
    });

    expect(() => verifyKybWebhook({
      timestamp: vector.timestamp,
      signature: vector.signature,
      rawBody: vector.rawBody,
    }, { ...makeConfig(), maxClockSkewSeconds: 30 })).toThrow("outside allowed clock skew");
    expect(() => verifyKybWebhook({
      timestamp: vector.timestamp,
      signature: "not-a-signature",
      rawBody: vector.rawBody,
    }, makeConfig())).toThrow("Malformed");
    expect(() => verifyKybWebhook({
      timestamp: vector.timestamp,
      signature: vector.signature,
      rawBody: `${vector.rawBody} `,
    }, makeConfig())).toThrow("Invalid KYB webhook signature");
  });

  it("fails closed when the replay cache is full", () => {
    const replayCache = new InMemoryWebhookReplayCache(1);
    const config = { secret: vector.secret, replayCache, nowSeconds: () => nowSeconds };
    const first = { timestamp: vector.timestamp, signature: vector.signature, rawBody: vector.rawBody };
    verifyKybWebhook(first, config);
    const nextTimestamp = String(nowSeconds);
    const nextSignature = createHmac("sha256", vector.secret)
      .update(`${nextTimestamp}.${vector.rawBody}`)
      .digest("hex");
    expect(() => verifyKybWebhook({
      timestamp: nextTimestamp,
      signature: nextSignature,
      rawBody: vector.rawBody,
    }, config)).toThrow("capacity reached");
  });
});
