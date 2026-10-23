/**
 * Wave Service
 * Fetches and processes Wave contributor data from GitHub and on-chain treasury
 */

import {
  WaveIssue,
  WaveIssueStatus,
  PayoutStatus,
  Contributor,
  GrantDisbursement,
  WaveStats,
} from '../types/wave';

const GITHUB_API_BASE = 'https://api.github.com';
export const GITHUB_REPO = process.env.GITHUB_REPO || 'OpenLedger-Foundation/Kora-Contract';
const INDEXER_API_BASE = process.env.INDEXER_API_BASE || 'https://api.kora.finance';
const GITHUB_TOKEN = process.env.GITHUB_TOKEN;

export class WaveService {
  private etag: string | null = null;
  private cachedIssues: WaveIssue[] | null = null;

  /**
   * Build GitHub issues URL
   */
  getIssuesUrl(): string {
    return `${GITHUB_API_BASE}/repos/${GITHUB_REPO}/issues?labels=wave,contributor-reward&state=all&per_page=100`;
  }

  private githubHeaders(): Record<string, string> {
    const headers: Record<string, string> = { Accept: 'application/vnd.github+json' };
    if (GITHUB_TOKEN) headers.Authorization = `Bearer ${GITHUB_TOKEN}`;
    if (this.etag) headers['If-None-Match'] = this.etag;
    return headers;
  }

  /**
   * Fetch all Wave issues from GitHub
   */
  async fetchWaveIssues(): Promise<WaveIssue[]> {
    try {
      const response = await fetch(this.getIssuesUrl(), { headers: this.githubHeaders() });

      if (response.status === 304 && this.cachedIssues) {
        return this.cachedIssues;
      }

      if (!response.ok) {
        throw new Error(`GitHub API error: ${response.statusText}`);
      }

      this.etag = response.headers.get('etag');
      const issues = await response.json();
      const parsed = issues.map((issue: any) => this.parseGitHubIssue(issue));
      this.cachedIssues = parsed;
      return parsed;
    } catch (error) {
      console.error('Failed to fetch Wave issues:', error);
      return [];
    }
  }

  /**
   * Parse GitHub issue into WaveIssue format
   */
  private parseGitHubIssue(issue: any): WaveIssue {
    const complexityLabel = issue.labels.find((l: any) =>
      ['Low', 'Medium', 'High'].includes(l.name)
    );
    const complexity = complexityLabel?.name || 'Medium';
    const pointValue = this.getPointsForComplexity(complexity);

    const status = this.parseIssueStatus(issue);
    const payoutStatus = this.parsePayoutStatus(issue);

    return {
      id: `issue-${issue.number}`,
      issueNumber: issue.number,
      title: issue.title,
      description: issue.body || '',
      complexity,
      pointValue,
      status,
      claimedBy: issue.assignee?.login,
      claimedAt: issue.assignee ? new Date(issue.updated_at) : undefined,
      prNumber: this.extractPRNumber(issue),
      mergedAt: issue.closed_at && status === 'completed' ? new Date(issue.closed_at) : undefined,
      payoutStatus,
      grantProposalId: this.extractGrantProposalId(issue),
      createdAt: new Date(issue.created_at),
      updatedAt: new Date(issue.updated_at),
    };
  }

  /**
   * Get point value based on complexity
   */
  private getPointsForComplexity(complexity: string): number {
    switch (complexity) {
      case 'Low':
        return 50;
      case 'Medium':
        return 100;
      case 'High':
        return 200;
      default:
        return 100;
    }
  }

  /**
   * Parse issue status from GitHub issue state and labels
   */
  private parseIssueStatus(issue: any): WaveIssueStatus {
    if (issue.state === 'closed') {
      const labels = issue.labels.map((l: any) => l.name);
      if (labels.includes('completed')) return WaveIssueStatus.COMPLETED;
      if (labels.includes('abandoned')) return WaveIssueStatus.ABANDONED;
      return WaveIssueStatus.COMPLETED;
    }

    if (issue.assignee) {
      const labels = issue.labels.map((l: any) => l.name);
      if (labels.includes('in-progress')) return WaveIssueStatus.IN_PROGRESS;
      return WaveIssueStatus.CLAIMED;
    }

    return WaveIssueStatus.OPEN;
  }

  /**
   * Parse payout status from issue labels and body
   */
  private parsePayoutStatus(issue: any): PayoutStatus {
    const labels = issue.labels.map((l: any) => l.name);
    const body = issue.body?.toLowerCase() || '';

    if (labels.includes('payout-disbursed') || body.includes('disbursed')) {
      return PayoutStatus.DISBURSED;
    }
    if (labels.includes('payout-approved')) {
      return PayoutStatus.APPROVED;
    }
    if (labels.includes('not-eligible')) {
      return PayoutStatus.NOT_ELIGIBLE;
    }
    if (labels.includes('payout-rejected')) {
      return PayoutStatus.REJECTED;
    }
    if (issue.state === 'closed' && labels.includes('completed')) {
      return PayoutStatus.PENDING;
    }

    return PayoutStatus.PENDING;
  }

  /**
   * Extract PR number from issue body or timeline
   */
  private extractPRNumber(issue: any): number | undefined {
    const body = issue.body || '';
    const prMatch = body.match(/Closes #(\d+)|Fixes #(\d+)|PR: #(\d+)/i);
    return prMatch ? parseInt(prMatch[1] || prMatch[2] || prMatch[3]) : undefined;
  }

  /**
   * Extract grant proposal ID from issue body
   */
  private extractGrantProposalId(issue: any): number | undefined {
    const body = issue.body || '';
    const proposalMatch = body.match(/Grant Proposal: #(\d+)|Proposal ID: (\d+)/i);
    return proposalMatch ? parseInt(proposalMatch[1] || proposalMatch[2]) : undefined;
  }

  /**
   * Fetch grant disbursements from on-chain treasury via indexer
   */
  async fetchGrantDisbursements(): Promise<GrantDisbursement[]> {
    try {
      const response = await fetch(`${INDEXER_API_BASE}/api/v1/treasury/grants?type=wave-contributor`);

      if (!response.ok) {
        throw new Error(`Indexer API error: ${response.statusText}`);
      }

      const data = await response.json();
      return data.grants.map((grant: any) => ({
        proposalId: grant.proposal_id,
        recipient: grant.recipient,
        amount: grant.amount,
        token: grant.token,
        issueNumber: grant.metadata?.issue_number,
        txHash: grant.tx_hash,
        timestamp: new Date(grant.timestamp),
      }));
    } catch (error) {
      console.error('Failed to fetch grant disbursements:', error);
      return [];
    }
  }

  /**
   * Fetch treasury payout totals from indexer endpoint
   */
  async fetchTreasuryPayouts(): Promise<{ totalDisbursed: number; totalPending: number }> {
    try {
      const response = await fetch(`${INDEXER_API_BASE}/api/v1/treasury/payouts?type=wave-contributor`, {
        headers: GITHUB_TOKEN ? { Authorization: `Bearer ${GITHUB_TOKEN}` } : {},
      });

      if (!response.ok) {
        throw new Error(`Indexer API error: ${response.statusText}`);
      }

      const data = await response.json();
      return {
        totalDisbursed: data.total_disbursed ?? data.totalDisbursed ?? 0,
        totalPending: data.total_pending ?? data.totalPending ?? 0,
      };
    } catch (error) {
      console.error('Failed to fetch treasury payouts:', error);
      const disbursements = await this.fetchGrantDisbursements();
      const totalDisbursed = disbursements.reduce((sum, d) => sum + Number(d.amount ?? 0), 0);
      return { totalDisbursed, totalPending: 0 };
    }
  }
  async fetchWaveIssuesWithDisbursements(): Promise<WaveIssue[]> {
    const [issues, disbursements] = await Promise.all([
      this.fetchWaveIssues(),
      this.fetchGrantDisbursements(),
    ]);

    const disbursementMap = new Map<number, GrantDisbursement>();
    disbursements.forEach((d) => {
      if (d.issueNumber) {
        disbursementMap.set(d.issueNumber, d);
      }
    });

    return issues.map((issue) => {
      const disbursement = disbursementMap.get(issue.issueNumber);
      if (disbursement) {
        return {
          ...issue,
          grantProposalId: disbursement.proposalId,
          disbursementTxHash: disbursement.txHash,
          payoutStatus: PayoutStatus.DISBURSED,
        };
      }
      return issue;
    });
  }

  /**
   * Aggregate contributor statistics
   */
  async fetchContributors(): Promise<Contributor[]> {
    const issues = await this.fetchWaveIssuesWithDisbursements();
    const contributorMap = new Map<string, Contributor>();

    issues.forEach((issue) => {
      if (!issue.claimedBy) return;

      const existing = contributorMap.get(issue.claimedBy);
      if (!existing) {
        contributorMap.set(issue.claimedBy, {
          address: issue.claimedBy,
          githubUsername: issue.claimedBy,
          totalIssues: 1,
          completedIssues: issue.status === WaveIssueStatus.COMPLETED ? 1 : 0,
          totalPoints: issue.status === WaveIssueStatus.COMPLETED ? issue.pointValue : 0,
          totalPayout: issue.payoutStatus === PayoutStatus.DISBURSED ? issue.pointValue : 0,
          pendingPayout:
            issue.status === WaveIssueStatus.COMPLETED &&
            issue.payoutStatus !== PayoutStatus.DISBURSED
              ? issue.pointValue
              : 0,
          joinedAt: issue.claimedAt!,
        });
      } else {
        existing.totalIssues += 1;
        if (issue.status === WaveIssueStatus.COMPLETED) {
          existing.completedIssues += 1;
          existing.totalPoints += issue.pointValue;
        }
        if (issue.payoutStatus === PayoutStatus.DISBURSED) {
          existing.totalPayout += issue.pointValue;
        }
        if (
          issue.status === WaveIssueStatus.COMPLETED &&
          issue.payoutStatus !== PayoutStatus.DISBURSED
        ) {
          existing.pendingPayout += issue.pointValue;
        }
      }
    });

    return Array.from(contributorMap.values()).sort((a, b) => b.totalPoints - a.totalPoints);
  }

  /**
   * Calculate Wave statistics
   */
  async fetchWaveStats(): Promise<WaveStats> {
    const [issues, contributors] = await Promise.all([
      this.fetchWaveIssuesWithDisbursements(),
      this.fetchContributors(),
    ]);

    return {
      totalIssues: issues.length,
      openIssues: issues.filter((i) => i.status === WaveIssueStatus.OPEN).length,
      claimedIssues: issues.filter((i) =>
        [WaveIssueStatus.CLAIMED, WaveIssueStatus.IN_PROGRESS].includes(i.status)
      ).length,
      completedIssues: issues.filter((i) => i.status === WaveIssueStatus.COMPLETED).length,
      totalPointsAllocated: issues
        .filter((i) => i.status === WaveIssueStatus.COMPLETED)
        .reduce((sum, i) => sum + i.pointValue, 0),
      totalDisbursed: issues
        .filter((i) => i.payoutStatus === PayoutStatus.DISBURSED)
        .reduce((sum, i) => sum + i.pointValue, 0),
      activeContributors: contributors.length,
    };
  }
}

export const waveService = new WaveService();
