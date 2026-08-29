export function simulateFee(amount: bigint, feeBps: number) {
  return (amount * BigInt(feeBps)) / 10_000n;
}

export function simulateYield(principal: bigint, yieldBps: number) {
  return principal + simulateFee(principal, yieldBps);
}
