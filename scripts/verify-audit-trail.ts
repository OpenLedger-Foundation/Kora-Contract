import { readFileSync } from "node:fs";

export function verifyAuditTrail(path: string) {
  const source = readFileSync(path, "utf8");
  return source.includes("AdminAuditEntry");
}

if (process.argv[2]) {
  console.log(verifyAuditTrail(process.argv[2]) ? "ok" : "missing");
}
