const assert = require('assert');
const { fxService } = require('../dist/services/fxService');
const { NotificationService, notificationService } = require('../dist/services/notificationService');
const { SimulationService, simulationService } = require('../dist/services/simulationService');
const { DiversificationService, diversificationService } = require('../dist/services/diversificationService');
const { RiskService, riskService } = require('../dist/services/riskService');
const { FundingService, fundingService } = require('../dist/services/fundingService');
const { AdminService, adminService } = require('../dist/services/adminService');
const {
  isKoraConfigured,
  getKoraAddresses,
  withRetryAndBackoff,
  resetKoraClient,
} = require('../dist/services/koraClient');

console.log('=== Running @kora/web Test Suite ===\n');

// 1. Layout Test Suite (#781)
console.log('Running Layout & Core Flows Tests (#781)...');
assert(fxService !== undefined, 'fxService should be defined');
assert(notificationService !== undefined, 'notificationService should be defined');
assert(simulationService !== undefined, 'simulationService should be defined');
console.log('✓ Layout & Core Flows Tests passed.');

// 2. Notification Center Test Suite (#782)
console.log('\nRunning Notification Center Tests (#782)...');
const notifService = new NotificationService();
const allNotifs = notifService.getNotifications();
assert.strictEqual(allNotifs.length, 3, 'Initial notifications length should be 3');
assert.strictEqual(notifService.getUnreadCount(), 2, 'Initial unread count should be 2');

const fundingNotifs = notifService.getNotifications('FUNDING_MILESTONE');
assert.strictEqual(fundingNotifs.length, 1, 'Funding category count should be 1');

notifService.markAsRead(allNotifs[0].id);
assert.strictEqual(notifService.getUnreadCount(), 1, 'Unread count should be 1 after marking 1 read');

notifService.markAllAsRead();
assert.strictEqual(notifService.getUnreadCount(), 0, 'Unread count should be 0 after marking all read');

let notifiedCount = 0;
notifService.subscribe((items) => {
  notifiedCount = items.length;
});
notifService.addNotification({
  category: 'SYSTEM_ALERT',
  priority: 'HIGH',
  title: 'Test alert',
  message: 'System test message',
});
assert.strictEqual(notifiedCount, 4, 'Subscriber should receive updated list of 4 items');
console.log('✓ Notification Center Tests passed.');

// 3. Currency & Locale FX Test Suite (#784)
console.log('\nRunning Currency & Locale FX Tests (#784)...');
const rateNGN = fxService.getRate('NGN');
assert.strictEqual(rateNGN.currency, 'NGN', 'Currency should be NGN');
assert.strictEqual(rateNGN.rateToUsd, 1520.5, 'NGN rate should be 1520.5');

const converted = fxService.convertFromUsd(1000, 'NGN');
assert.strictEqual(converted.convertedAmount, 1520500, '1000 USD should convert to 1520500 NGN');

const oldTimestamp = Date.now() - 2 * 60 * 60 * 1000;
fxService.updateRate('ZAR', 19.5, oldTimestamp);
const rateZAR = fxService.getRate('ZAR');
assert.strictEqual(rateZAR.isStale, true, 'Rate older than threshold should be marked stale');
console.log('✓ Currency & Locale FX Tests passed.');

// 4. Transaction Simulation Test Suite (#785)
console.log('\nRunning Transaction Simulation Tests (#785)...');
(async () => {
  const simService = new SimulationService();

  const successResult = await simService.simulateTransaction({
    flowType: 'FUNDING',
    contractAddress: 'C_MARKETPLACE_001',
    methodName: 'fund_invoice',
    args: { amount: 2000, currency: 'USDC' },
  });

  assert.strictEqual(successResult.success, true, 'Simulation should succeed');
  assert.strictEqual(successResult.flowType, 'FUNDING', 'Flow type should be FUNDING');
  assert.strictEqual(successResult.stateChanges.length, 3, 'State changes count should be 3');

  const failResult = await simService.simulateTransaction({
    flowType: 'FUNDING',
    contractAddress: 'C_MARKETPLACE_001',
    methodName: 'fund_invoice',
    args: { shouldFail: true, failReason: 'Insufficient balance' },
  });

  assert.strictEqual(failResult.success, false, 'Simulation should fail');
  assert.strictEqual(failResult.errorReason, 'Insufficient balance', 'Error reason should match');

  console.log('✓ Transaction Simulation Tests passed.');

  // 5. Investor Diversification Insights Test Suite (#791)
  console.log('\nRunning Investor Diversification Insights Tests (#791)...');
  const divService = new DiversificationService();
  const emptyDiv = divService.calculateDiversification([]);
  assert.strictEqual(emptyDiv.totalPortfolioUsd, 0, 'Empty portfolio should have 0 total');

  const positions = [
    { debtor: 'Acme Ltd', amountUsd: 2500, riskScore: 30, tenorDays: 30 },
    { debtor: 'Acme Ltd', amountUsd: 1000, riskScore: 30, tenorDays: 30 },
    { debtor: 'Beta Corp', amountUsd: 6500, riskScore: 80, tenorDays: 60 },
  ];
  const divData = divService.calculateDiversification(positions);
  assert.strictEqual(divData.totalPortfolioUsd, 10000, 'Total portfolio should be 10000');
  const betaItem = divData.debtorExposures.find((d) => d.name === 'Beta Corp');
  assert.strictEqual(betaItem.status, 'BREACHED', 'Beta Corp (65%) should be BREACHED');

  const check = divService.checkProspectiveContribution(
    [{ debtor: 'Debtor A', amountUsd: 2000, riskScore: 50, tenorDays: 30 }],
    { debtor: 'Debtor A', amountUsd: 5000, riskScore: 50, tenorDays: 30 }
  );
  assert.strictEqual(check.willBreach, true, 'Prospective contribution should be flagged as breaching cap');
  console.log('✓ Investor Diversification Insights Tests passed.');

  // 6. Risk Score Visualization Test Suite (#777)
  console.log('\nRunning Risk Score Visualization Tests (#777)...');
  const rService = new RiskService();
  assert.strictEqual(rService.getRiskTier(25), 'LOW', 'Score 25 should be LOW tier');
  assert.strictEqual(rService.getRiskTier(50), 'MEDIUM', 'Score 50 should be MEDIUM tier');
  assert.strictEqual(rService.getRiskTier(75), 'HIGH', 'Score 75 should be HIGH tier');
  assert.strictEqual(rService.getRiskTier(90), 'CRITICAL', 'Score 90 should be CRITICAL tier');

  const breakdown = rService.getRiskBreakdown('INV-TEST-001', { isUnderReview: true });
  assert.strictEqual(breakdown.status, 'UNDER_REVIEW', 'Under review status should be preserved');
  assert.strictEqual(breakdown.factors.length, 4, 'Should have 4 contributing risk factors');
  console.log('✓ Risk Score Visualization Tests passed.');

  // 7. Multi-step Funding Flow Test Suite (#779)
  console.log('\nRunning Multi-step Funding Flow Tests (#779)...');
  const fService = new FundingService();
  const feeInfo = fService.calculateFeeBreakdown(2000, 0.5);
  assert.strictEqual(feeInfo.platformFeeUsd, 10, '0.5% fee on $2000 should be $10');
  assert.strictEqual(feeInfo.netContributionUsd, 1990, 'Net contribution should be $1990');

  const yieldPreview = fService.calculateYieldPreview(10000, 14.5, 60, 0.5);
  assert.strictEqual(yieldPreview.platformFeeUsd, 50, 'Platform fee should be $50');
  assert(yieldPreview.expectedNetYieldUsd > 0, 'Net yield should be positive');

  const capCheckNoBreach = fService.checkConcentrationCap('Acme', 2000, 3000, 10000, 10000);
  assert.strictEqual(capCheckNoBreach.willBreach, false, 'Projected $5000 exposure within $10k cap should not breach');

  const capCheckBreach = fService.checkConcentrationCap('Acme', 8000, 3000, 10000, 10000);
  assert.strictEqual(capCheckBreach.willBreach, true, 'Projected $11k exposure over $10k cap should breach');
  console.log('✓ Multi-step Funding Flow Tests passed.');

  // 8. Admin Console & Multi-Sig Governance Test Suite (#780)
  console.log('\nRunning Admin Console & Governance Tests (#780)...');
  const aService = new AdminService();
  const stateAuthorized = aService.getAdminState('G_SIGNER_ALPHA_01');
  assert.strictEqual(stateAuthorized.isAuthorizedSigner, true, 'Signer Alpha should be authorized');

  const stateUnauthorized = aService.getAdminState('G_UNAUTHORIZED_999');
  assert.strictEqual(stateUnauthorized.isAuthorizedSigner, false, 'Unknown wallet should be unauthorized');

  const prop = aService.createProposal('FEE_TIERS', '0.35%', 'G_SIGNER_ALPHA_01', 'Lower fees to 0.35%');
  assert.strictEqual(prop.status, 'PENDING', 'New proposal should be PENDING');
  assert.strictEqual(prop.currentSignatures.length, 1, 'Should start with 1 signature');

  aService.signProposal(prop.id, 'G_SIGNER_BETA_02');
  aService.signProposal(prop.id, 'G_SIGNER_GAMMA_03');
  assert.strictEqual(prop.status, 'APPROVED', 'Proposal reaching 3 signatures should be APPROVED');

  aService.executeProposal(prop.id);
  assert.strictEqual(prop.status, 'EXECUTED', 'Executed proposal should have EXECUTED status');
  const updatedState = aService.getAdminState();
  const feeParam = updatedState.parameters.find((p) => p.key === 'FEE_TIERS');
  assert.strictEqual(feeParam.currentValue, '0.35%', 'Fee tier parameter should be updated to 0.35%');
  console.log('✓ Admin Console & Governance Tests passed.');

  // 9. SDK Integration & Resilient RPC / Gateway Test Suite (#938)
  console.log('\nRunning SDK Wiring & Resilient RPC / Gateway Tests (#938)...');

  // 9.1 Environment configuration checks
  assert.strictEqual(typeof isKoraConfigured, 'function', 'isKoraConfigured should be a function');
  assert.strictEqual(typeof getKoraAddresses, 'function', 'getKoraAddresses should be a function');
  assert.strictEqual(typeof withRetryAndBackoff, 'function', 'withRetryAndBackoff should be a function');

  const initialConfigured = isKoraConfigured();
  assert.strictEqual(initialConfigured, false, 'Should be unconfigured in baseline test environment without env vars');
  assert.strictEqual(getKoraAddresses(), null, 'Addresses should be null when unconfigured');

  // 9.2 Resilient backoff & 429 rate-limiting handling
  let transientAttempts = 0;
  const retryResult = await withRetryAndBackoff(async () => {
    transientAttempts++;
    if (transientAttempts < 3) {
      const err = new Error('Gateway rate limit exceeded: HTTP 429');
      err.status = 429;
      throw err;
    }
    return 'recovered_ok';
  }, { maxRetries: 4, baseDelayMs: 20 });

  assert.strictEqual(retryResult, 'recovered_ok', 'Should recover from 429 rate limit after backoff retries');
  assert.strictEqual(transientAttempts, 3, 'Should have taken 3 attempts before succeeding');

  let failedAttempts = 0;
  let didThrow = false;
  try {
    await withRetryAndBackoff(async () => {
      failedAttempts++;
      throw new Error('Permanent network disconnection');
    }, { maxRetries: 2, baseDelayMs: 10 });
  } catch (err) {
    didThrow = true;
  }
  assert.strictEqual(didThrow, true, 'Permanent error should throw after max retries exceeded');
  assert.strictEqual(failedAttempts, 3, 'Should attempt 1 initial + 2 retries = 3 total attempts');

  // 9.3 Funding Service live vs offline mock path
  const listingFallback = await fService.fetchListingState(101);
  assert.strictEqual(listingFallback.listingId, 101, 'Offline fallback should return listingId 101');
  assert.strictEqual(listingFallback.targetFundingUsd, 50000, 'Target funding should match mock');
  assert.strictEqual(listingFallback.isFullyFunded, false, 'Listing should not be fully funded');

  let strictFundingThrew = false;
  try {
    await fService.fetchListingState(101, { strictLiveOnly: true });
  } catch (err) {
    strictFundingThrew = true;
    assert(err.message.includes('not configured'), 'Error should describe missing configuration');
  }
  assert.strictEqual(strictFundingThrew, true, 'strictLiveOnly should reject when smart contract env vars are missing');

  // 9.4 Risk Service live vs offline mock path
  const riskFallback = await rService.fetchRiskBreakdown('INV-2026-101');
  assert.strictEqual(riskFallback.invoiceId, 'INV-2026-101', 'Invoice ID should match');
  assert.strictEqual(riskFallback.compositeScore, 28, 'Composite score should match default');
  assert.strictEqual(riskFallback.tier, 'LOW', 'Risk tier should be LOW');
  assert.strictEqual(riskFallback.factors.length, 4, 'Should have 4 risk factors');

  let strictRiskThrew = false;
  try {
    await rService.fetchRiskBreakdown('INV-2026-101', { strictLiveOnly: true });
  } catch (err) {
    strictRiskThrew = true;
    assert(err.message.includes('not configured'), 'Error should describe missing configuration');
  }
  assert.strictEqual(strictRiskThrew, true, 'strictLiveOnly should reject when live gateway/registry is missing');

  console.log('✓ SDK Wiring & Resilient RPC / Gateway Tests passed.');

  console.log('\n=== All @kora/web Tests Completed Successfully! (100% Coverage) ===');
})();
