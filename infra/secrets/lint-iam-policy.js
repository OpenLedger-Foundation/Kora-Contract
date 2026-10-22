#!/usr/bin/env node
/**
 * Lint IAM policies against infra/secrets/README.md path conventions.
 * Usage: node lint-iam-policy.js [--policy <path>] [--strict]
 * Exit 0 when all statements grant only GetSecretValue/DescribeSecret on
 * /kora/ENV/SERVICE paths; non-zero with concise errors (no secret values).
 */
const fs = require("fs");
const path = require("path");

const ALLOWED_ACTIONS = new Set([
  "secretsmanager:GetSecretValue",
  "secretsmanager:DescribeSecret",
]);

function fail(msg) {
  console.error(`IAM-LINT FAIL: ${msg}`);
  process.exitCode = 1;
}

function main() {
  const args = process.argv.slice(2);
  const pi = args.indexOf("--policy");
  const policyPath = pi >= 0 && args[pi + 1]
    ? args[pi + 1]
    : path.join(__dirname, "iam-policy.example.json");
  let policy;
  try {
    policy = JSON.parse(fs.readFileSync(policyPath, "utf8"));
  } catch (e) {
    fail(`cannot read policy ${policyPath}: ${e.message}`);
    return;
  }
  const statements = policy.Statement || [];
  if (!Array.isArray(statements) || statements.length === 0) {
    fail("no Statement array entries found");
    return;
  }
  for (const [i, st] of statements.entries()) {
    const actions = Array.isArray(st.Action) ? st.Action : [st.Action];
    for (const a of actions) {
      if (!ALLOWED_ACTIONS.has(a)) fail(`statement[${i}] disallowed Action ${a}`);
    }
    const resources = Array.isArray(st.Resource) ? st.Resource : [st.Resource];
    for (const r of resources) {
      if (typeof r !== "string") { fail(`statement[${i}] non-string Resource`); continue; }
      if (!r.includes("/kora/")) fail(`statement[${i}] Resource missing /kora/ prefix: redacted`);
      // Reject wildcard overreach: only the trailing 6-char suffix wildcard is allowed.
      const afterKora = r.split("/kora/")[1] || "";
      const stars = (r.match(/\*/g) || []).length;
      if (stars > 1 || (stars === 1 && !/-\?\?\?\?\?\?$/.test(r))) {
        fail(`statement[${i}] wildcard overreach beyond service slug placeholder`);
      }
      if (afterKora.includes("*") && !/-\?\?\?\?\?\?$/.test(r)) {
        fail(`statement[${i}] wildcard inside secret path`);
      }
    }
  }
  if (!process.exitCode) console.log("IAM-LINT PASS");
}

if (require.main === module) main();
module.exports = {};
