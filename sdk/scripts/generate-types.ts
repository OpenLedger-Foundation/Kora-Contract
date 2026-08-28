import { writeFileSync } from "node:fs";

const generated = `export type ContractSpecName = "invoice_nft" | "marketplace" | "price_oracle" | "risk_registry";\n`;
writeFileSync("sdk/src/generated-types.ts", generated);
