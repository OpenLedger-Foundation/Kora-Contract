import { PriceOracleClient, RiskRegistryClient } from "./clients";

export function createOracleRiskHelpers(priceOracle: PriceOracleClient, riskRegistry: RiskRegistryClient) {
  return { priceOracle, riskRegistry };
}
