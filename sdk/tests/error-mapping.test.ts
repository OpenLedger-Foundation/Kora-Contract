import * as fs from "fs";
import * as path from "path";
import { CONTRACT_ERROR_CODES, KoraErrorCode } from "../src/contractErrors";
import { mapContractError } from "../src/errorMapping";
// eslint-disable-next-line @typescript-eslint/no-var-requires
const { generate } = require("../scripts/generate-errors.js");

describe("mapContractError", () => {
  it("is generated from the current contract enums", () => {
    expect(fs.readFileSync(path.join(__dirname, "../src/contractErrors.ts"), "utf8")).toBe(generate());
  });
  it("maps every declared code", () => {
    for (const [contract, table] of Object.entries(CONTRACT_ERROR_CODES)) {
      for (const [n, code] of Object.entries(table)) {
        expect(mapContractError(Number(n), contract).code).toBe(code);
      }
    }
  });
  it("accepts numbers, strings, errors and contract name styles", () => {
    expect(mapContractError("HostError: Error(Contract, #1)", "shared").code).toBe(KoraErrorCode.Unauthorized);
    expect(mapContractError(new Error("Error(Contract, #1)"), "shared").code).toBe(KoraErrorCode.Unauthorized);
    const code = mapContractError(1, "secondary_market").code;
    expect(mapContractError(1, "SecondaryMarket").code).toBe(code);
    expect(mapContractError(1, "secondary-market").code).toBe(code);
  });
  it("maps marketplace through the shared registry", () => {
    expect(mapContractError(20, "marketplace").code).toBe(KoraErrorCode.ListingNotFound);
  });
  it("marks paused retryable and auth failures not", () => {
    expect(mapContractError(4, "shared")).toMatchObject({ code: KoraErrorCode.ProtocolPaused, retryable: true });
    expect(mapContractError(1, "shared").retryable).toBe(false);
  });
  it.each([[99999, "shared"], [1, "nope"], [1, "constructor"], [undefined, "shared"], [null, "shared"], ["timeout", "shared"], [1.5, "shared"], [{}, ""]])(
    "fails closed for %p on %s", (e, c) => {
      expect(mapContractError(e, c as string)).toMatchObject({ code: KoraErrorCode.Unknown, retryable: false });
    });
});
