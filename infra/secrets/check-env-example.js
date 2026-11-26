#!/usr/bin/env node
/**
 * Fail when infra/secrets/.env.example drifts from the secrets services read.
 * Usage: node check-env-example.js [--services <dir>] [--example <file>]
 * Only key names are printed, never values.
 */
const fs = require("fs");
const path = require("path");

const SECRET_NAME = /(^|_)(SECRETS?|KEYS?|TOKENS?|PASSWORDS?|MNEMONIC)$/;
const ENV_READ = /\benv(?:\.([A-Z][A-Z0-9_]*)|\[\s*["']([A-Z][A-Z0-9_]*)["']\s*\]|\(\s*["']([A-Z][A-Z0-9_]*)["'])|env::var\(\s*"([A-Z][A-Z0-9_]*)"/g;
const SKIP = new Set(["node_modules", "dist", "coverage", "target", "tests", "__tests__"]);

function requiredSecrets(dir, found = new Set()) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (!SKIP.has(entry.name)) requiredSecrets(full, found);
    } else if (/\.(ts|js|rs)$/.test(entry.name)) {
      for (const m of fs.readFileSync(full, "utf8").matchAll(ENV_READ)) {
        const key = m.slice(1).find(Boolean);
        if (SECRET_NAME.test(key)) found.add(key);
      }
    }
  }
  return found;
}

function checkExample(text, required) {
  const errors = [];
  const seen = new Set();
  text.split(/\r?\n/).forEach((raw, i) => {
    const line = raw.trim();
    if (!line || line.startsWith("#")) return;
    const m = /^(?:export\s+)?([A-Z][A-Z0-9_]*)=(.*)$/.exec(line);
    if (!m) return errors.push(`line ${i + 1}: malformed entry`);
    if (seen.has(m[1])) errors.push(`${m[1]}: duplicate key`);
    seen.add(m[1]);
    if (m[2] !== "") errors.push(`${m[1]}: must not have a value`);
  });
  for (const k of [...required].sort()) if (!seen.has(k)) errors.push(`${k}: missing from example`);
  for (const k of seen) if (!required.has(k)) errors.push(`${k}: stale, no service reads it`);
  if (required.size === 0) errors.push("no runtime secrets found; check the services path");
  return errors;
}

function main() {
  const args = process.argv.slice(2);
  const opt = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
  let errors;
  try {
    const services = opt("--services", path.join(__dirname, "../../services"));
    const example = opt("--example", path.join(__dirname, ".env.example"));
    errors = checkExample(fs.readFileSync(example, "utf8"), requiredSecrets(services));
  } catch (e) {
    errors = [e.message];
  }
  errors.forEach((e) => console.error(`ENV-DRIFT FAIL: ${e}`));
  if (errors.length) process.exitCode = 1;
  else console.log("env example OK");
}

if (require.main === module) main();
module.exports = { requiredSecrets, checkExample };
