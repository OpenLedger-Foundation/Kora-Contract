# KYB webhook verification

Before parsing or acting on a provider webhook, pass the **exact raw request
body** to `verifyKybWebhook` with the provider's timestamp and HMAC-SHA256
signature. The signed message format is `${timestamp}.${rawBody}` and the
signature may be supplied as 64 hex characters or `sha256=<hex>`.

The verifier rejects malformed signatures, timestamps outside the default
five-minute skew window, invalid MACs, and repeated signed deliveries. It
compares MACs in constant time and only inserts into the replay cache after
signature validation. Do not JSON-parse and reserialize the body before
verification.

`InMemoryWebhookReplayCache` is suitable only for tests or a single process.
Multi-replica deployments must inject a shared cache whose `addIfAbsent`
operation is atomic (for example, a Redis `SET NX` with expiry). Cache
unavailability or capacity exhaustion must reject the webhook; do not fall
back to an unverified or replay-permitting path. Keep the signing secret in
`/kora/<environment>/kyb` and never log it or the signature.

Example:

```ts
verifyKybWebhook(
  { timestamp: request.headers["x-kyb-timestamp"], signature: request.headers["x-kyb-signature"], rawBody },
  { secret: process.env.KYB_WEBHOOK_SECRET!, replayCache },
);
```
