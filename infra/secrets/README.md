# Backend secrets management

Production and testnet service credentials belong in AWS Secrets Manager, not in
`.env` files, container images, deployment manifests, or command-line arguments.
Use a separate AWS account per environment where practical; otherwise use
separate KMS keys and the environment-qualified secret paths below. Workloads
receive credentials through their own task/pod identity and inject them as
environment variables at process start. The service code does not need AWS
credentials or permission to list secrets.

## Secret ownership and paths

Use one secret per service and environment, with only that service's fields in
the JSON value. Names are case-sensitive:

| Environment | Secret name | Service-only contents |
|---|---|---|
| `dev` | `/kora/dev/keeper` | Keeper Stellar signing key and RPC credential, if required |
| `testnet` | `/kora/testnet/keeper` | Testnet keeper signing key and testnet RPC credential |
| `production` | `/kora/production/keeper` | Production keeper signing key and production RPC credential |
| `dev` | `/kora/dev/fx-ingestion` | FX provider API keys and oracle relay signing key |
| `testnet` | `/kora/testnet/fx-ingestion` | Testnet FX provider API keys and oracle relay signing key |
| `production` | `/kora/production/fx-ingestion` | Production FX provider API keys and oracle relay signing key |
| `dev`, `testnet`, `production` | `/kora/<environment>/api` | API-specific RPC or third-party credentials only |
| `dev`, `testnet`, `production` | `/kora/<environment>/pinning` | Pinning-provider credential only, if that service is deployed |
| `dev`, `testnet`, `production` | `/kora/<environment>/kyb` | KYB encryption key and provider credentials only, if deployed |

Do not copy secrets between environments. Keep contract addresses, public RPC
URLs, and other non-secret configuration in normal deployment configuration.
The `scripts/contracts.env.example` file remains supported for public contract
addresses and must never contain secret values.

## Workload permissions and delivery

Create a distinct IAM role for each service deployment and environment. Grant
only `secretsmanager:GetSecretValue` and `secretsmanager:DescribeSecret` on that
workload's exact secret ARN, plus `kms:Decrypt` on the matching environment KMS
key when a customer-managed key is used. Do not grant `ListSecrets`, wildcard
secret resources, or cross-environment access. Bind the role using the
platform's workload identity (for example, an ECS task role or EKS Pod Identity
Association); never put AWS access keys in service configuration.

The deployment controller resolves the secret at task startup and exposes its
individual fields as the environment variables already consumed by the service
(for example, `KEEPER_SECRET`, `FX_RELAY_SECRET`, and `EXCHANGE_RATE_API_KEY`).
Use a rolling deployment to refresh a running process after updating a secret.
Do not log resolved values, include them in diagnostics, or write them to
health/status endpoints.

## Linting IAM policies

Validate committed policy snippets against the `/kora/ENV/SERVICE` path
convention before pushing:

```bash
node infra/secrets/lint-iam-policy.js --policy infra/secrets/iam-policy.example.json
```

The lint runs in `.github/workflows/security.yml` on pull requests. It only
allows `secretsmanager:GetSecretValue` and `secretsmanager:DescribeSecret`,
requires the `/kora/` prefix, and rejects `*` wildcards beyond the trailing
`-??????` service-slug placeholder. The negative fixture at
`infra/secrets/__fixtures__/bad-policy.json` must fail the lint.

## Local development

Local development does not need AWS or production access. Use disposable,
testnet-only credentials in shell-exported variables or an ignored local env
file outside the repository. Keep `scripts/contracts.env` limited to public
addresses; never place service credentials in it. The services consume
environment variables directly and intentionally do not load `.env` files.
Do not use production accounts or credentials in local development.

## Keeper key rotation without downtime

Rotate a production keeper key as a two-phase account-signer change. Keep the
old signer active until the replacement has successfully submitted a canary
transaction and the rolling deployment has converged:

1. Generate a new signing key in the approved custody system and store it as a
   new version of `/kora/production/keeper`. Do not overwrite or expose the old
   value; retain it as the previous secret version during the overlap.
2. Add the new public key as an authorized Stellar account signer and verify
   the account thresholds permit both old and new signers during the transition.
3. Promote the new secret version to `AWSCURRENT`, then perform a rolling
   keeper deployment. Existing tasks continue using the old key while new tasks
   start with the new key.
4. Confirm all tasks are healthy and the new key successfully submits a
   non-financial canary operation. Monitor transaction failures and keeper
   backlog through the monitoring dashboard before continuing.
5. Remove the old Stellar signer only after all tasks use the new key; then
   revoke/delete the old secret version according to the retention policy.
6. If the canary or rollout fails, restore the previous version as
   `AWSCURRENT` and roll tasks back before removing either account signer.

For an API token without overlapping credential support, provision a parallel
credential with the provider, deploy it, verify successful requests, and only
then revoke the old token. If the provider cannot overlap credentials, schedule
a maintenance window and document the expected interruption rather than
claiming a zero-downtime rotation.

## Audit and incident handling

Enable CloudTrail management/data events for secret reads and changes, and alert
on access outside the workload role, unexpected cross-environment reads, and
secret policy changes. Restrict write/rotation permissions to the operations
role; service roles are read-only. Treat suspected keeper-key exposure as
Sev-1 and follow [the incident response runbook](../../docs/INCIDENT_RESPONSE.md),
including admin/key custody escalation where applicable.