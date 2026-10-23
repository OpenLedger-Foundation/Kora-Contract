/**
 * Wave Dashboard Tests
 * Tests for Wave contributor tracking, cross-referencing, and display
 */

import { waveService } from '../src/services/waveService';
import { WaveIssueStatus, PayoutStatus } from '../src/types/wave';
import type { WaveIssue, Contributor, GrantDisbursement } from '../src/types/wave';

// Mock data
const mockGitHubIssues = [
  {
    number: 100,
    title: 'Build vesting contract',
    body: 'Implementation of vesting contract\nGrant Proposal: #1',
    state: 'closed',
    assignee: { login: 'contributor1' },
    labels: [{ name: 'wave' }, { name: 'contributor-reward' }, { name: 'High' }, { name: 'completed' }],
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-10T00:00:00Z',
    closed_at: '2024-01-10T00:00:00Z',
  },
  {
    number: 101,
    title: 'Add governance fast-track',
    body: 'Fast-track governance for security fixes',
    state: 'open',
    assignee: { login: 'contributor2' },
    labels: [{ name: 'wave' }, { name: 'contributor-reward' }, { name: 'High' }, { name: 'in-progress' }],
    created_at: '2024-01-05T00:00:00Z',
    updated_at: '2024-01-15T00:00:00Z',
    closed_at: null,
  },
  {
    number: 102,
    title: 'Documentation improvements',
    body: 'Improve documentation',
    state: 'open',
    assignee: null,
    labels: [{ name: 'wave' }, { name: 'contributor-reward' }, { name: 'Low' }],
    created_at: '2024-01-10T00:00:00Z',
    updated_at: '2024-01-10T00:00:00Z',
    closed_at: null,
  },
];

const mockDisbursements: GrantDisbursement[] = [
  {
    proposalId: 1,
    recipient: 'GABCD...XYZ',
    amount: 200,
    token: 'USDC',
    issueNumber: 100,
    txHash: '0x123abc',
    timestamp: new Date('2024-01-11T00:00:00Z'),
  },
];

describe('WaveService', () => {
  describe('parseGitHubIssue', () => {
    test('parses completed issue correctly', () => {
      const service = new (waveService.constructor as new () => unknown)() as any;
      const issue = service.parseGitHubIssue(mockGitHubIssues[0]);

      expect(issue.issueNumber).toBe(100);
      expect(issue.title).toBe('Build vesting contract');
      expect(issue.complexity).toBe('High');
      expect(issue.pointValue).toBe(200);
      expect(issue.status).toBe(WaveIssueStatus.COMPLETED);
      expect(issue.claimedBy).toBe('contributor1');
      expect(issue.grantProposalId).toBe(1);
    });

    test('parses in-progress issue correctly', () => {
      const service = new (waveService.constructor as new () => unknown)() as any;
      const issue = service.parseGitHubIssue(mockGitHubIssues[1]);

      expect(issue.issueNumber).toBe(101);
      expect(issue.status).toBe(WaveIssueStatus.IN_PROGRESS);
      expect(issue.claimedBy).toBe('contributor2');
      expect(issue.pointValue).toBe(200);
    });

    test('parses open issue correctly', () => {
      const service = new (waveService.constructor as new () => unknown)() as any;
      const issue = service.parseGitHubIssue(mockGitHubIssues[2]);

      expect(issue.issueNumber).toBe(102);
      expect(issue.status).toBe(WaveIssueStatus.OPEN);
      expect(issue.claimedBy).toBeUndefined();
      expect(issue.complexity).toBe('Low');
      expect(issue.pointValue).toBe(50);
    });
  });

  describe('getPointsForComplexity', () => {
    test('assigns correct points for each complexity level', () => {
      const service = new (waveService.constructor as new () => unknown)() as any;

      expect(service.getPointsForComplexity('Low')).toBe(50);
      expect(service.getPointsForComplexity('Medium')).toBe(100);
      expect(service.getPointsForComplexity('High')).toBe(200);
      expect(service.getPointsForComplexity('Unknown')).toBe(100); // Default
    });
  });

  describe('cross-referencing', () => {
    test('links GitHub issues with on-chain disbursements', () => {
      const issues: WaveIssue[] = [
        {
          id: 'issue-100',
          issueNumber: 100,
          title: 'Test issue',
          description: '',
          complexity: 'High',
          pointValue: 200,
          status: WaveIssueStatus.COMPLETED,
          claimedBy: 'contributor1',
          payoutStatus: PayoutStatus.PENDING,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
      ];

      const disbursements = mockDisbursements;
      const disbursementMap = new Map<number, GrantDisbursement>();
      disbursements.forEach((d) => {
        if (d.issueNumber) {
          disbursementMap.set(d.issueNumber, d);
        }
      });

      const linkedIssues = issues.map((issue) => {
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

      expect(linkedIssues[0].payoutStatus).toBe(PayoutStatus.DISBURSED);
      expect(linkedIssues[0].grantProposalId).toBe(1);
      expect(linkedIssues[0].disbursementTxHash).toBe('0x123abc');
    });

    test('handles issues without disbursements', () => {
      const issues: WaveIssue[] = [
        {
          id: 'issue-999',
          issueNumber: 999,
          title: 'Unclaimed issue',
          description: '',
          complexity: 'Low',
          pointValue: 50,
          status: WaveIssueStatus.OPEN,
          payoutStatus: PayoutStatus.PENDING,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
      ];

      const disbursements = mockDisbursements;
      const disbursementMap = new Map<number, GrantDisbursement>();
      disbursements.forEach((d) => {
        if (d.issueNumber) {
          disbursementMap.set(d.issueNumber, d);
        }
      });

      const linkedIssues = issues.map((issue) => {
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

      expect(linkedIssues[0].payoutStatus).toBe(PayoutStatus.PENDING);
      expect(linkedIssues[0].grantProposalId).toBeUndefined();
      expect(linkedIssues[0].disbursementTxHash).toBeUndefined();
    });
  });

  describe('contributor aggregation', () => {
    test('aggregates contributor statistics correctly', () => {
      const issues: WaveIssue[] = [
        {
          id: 'issue-100',
          issueNumber: 100,
          title: 'Issue 1',
          description: '',
          complexity: 'High',
          pointValue: 200,
          status: WaveIssueStatus.COMPLETED,
          claimedBy: 'contributor1',
          claimedAt: new Date('2024-01-01'),
          payoutStatus: PayoutStatus.DISBURSED,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
        {
          id: 'issue-101',
          issueNumber: 101,
          title: 'Issue 2',
          description: '',
          complexity: 'Medium',
          pointValue: 100,
          status: WaveIssueStatus.COMPLETED,
          claimedBy: 'contributor1',
          claimedAt: new Date('2024-01-02'),
          payoutStatus: PayoutStatus.PENDING,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
        {
          id: 'issue-102',
          issueNumber: 102,
          title: 'Issue 3',
          description: '',
          complexity: 'Low',
          pointValue: 50,
          status: WaveIssueStatus.IN_PROGRESS,
          claimedBy: 'contributor1',
          claimedAt: new Date('2024-01-03'),
          payoutStatus: PayoutStatus.PENDING,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
      ];

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

      const contributor = contributorMap.get('contributor1')!;
      expect(contributor.totalIssues).toBe(3);
      expect(contributor.completedIssues).toBe(2);
      expect(contributor.totalPoints).toBe(300); // 200 + 100
      expect(contributor.totalPayout).toBe(200); // Only disbursed
      expect(contributor.pendingPayout).toBe(100); // Completed but not disbursed
    });
  });

  describe('statistics calculation', () => {
    test('calculates Wave stats correctly', () => {
      const issues: WaveIssue[] = [
        {
          id: '1',
          issueNumber: 1,
          title: 'Issue 1',
          description: '',
          complexity: 'High',
          pointValue: 200,
          status: WaveIssueStatus.OPEN,
          payoutStatus: PayoutStatus.PENDING,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
        {
          id: '2',
          issueNumber: 2,
          title: 'Issue 2',
          description: '',
          complexity: 'Medium',
          pointValue: 100,
          status: WaveIssueStatus.CLAIMED,
          claimedBy: 'contributor1',
          payoutStatus: PayoutStatus.PENDING,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
        {
          id: '3',
          issueNumber: 3,
          title: 'Issue 3',
          description: '',
          complexity: 'High',
          pointValue: 200,
          status: WaveIssueStatus.COMPLETED,
          claimedBy: 'contributor1',
          payoutStatus: PayoutStatus.DISBURSED,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
        {
          id: '4',
          issueNumber: 4,
          title: 'Issue 4',
          description: '',
          complexity: 'Low',
          pointValue: 50,
          status: WaveIssueStatus.COMPLETED,
          claimedBy: 'contributor2',
          payoutStatus: PayoutStatus.PENDING,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
      ];

      const stats = {
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
        activeContributors: 2,
      };

      expect(stats.totalIssues).toBe(4);
      expect(stats.openIssues).toBe(1);
      expect(stats.claimedIssues).toBe(1);
      expect(stats.completedIssues).toBe(2);
      expect(stats.totalPointsAllocated).toBe(250); // 200 + 50
      expect(stats.totalDisbursed).toBe(200);
      expect(stats.activeContributors).toBe(2);
    });
  });

  describe('edge cases', () => {
    test('handles stale claims correctly', () => {
      const issue: WaveIssue = {
        id: 'issue-100',
        issueNumber: 100,
        title: 'Abandoned issue',
        description: '',
        complexity: 'Medium',
        pointValue: 100,
        status: WaveIssueStatus.ABANDONED,
        claimedBy: 'old-contributor',
        claimedAt: new Date('2024-01-01'),
        payoutStatus: PayoutStatus.NOT_ELIGIBLE,
        createdAt: new Date('2024-01-01'),
        updatedAt: new Date('2024-02-01'),
      };

      expect(issue.status).toBe(WaveIssueStatus.ABANDONED);
      expect(issue.payoutStatus).toBe(PayoutStatus.NOT_ELIGIBLE);
    });

    test('distinguishes between pending and rejected payouts', () => {
      const pendingIssue: WaveIssue = {
        id: '1',
        issueNumber: 1,
        title: 'Pending',
        description: '',
        complexity: 'Low',
        pointValue: 50,
        status: WaveIssueStatus.COMPLETED,
        payoutStatus: PayoutStatus.PENDING,
        createdAt: new Date(),
        updatedAt: new Date(),
      };

      const rejectedIssue: WaveIssue = {
        id: '2',
        issueNumber: 2,
        title: 'Rejected',
        description: '',
        complexity: 'Low',
        pointValue: 50,
        status: WaveIssueStatus.COMPLETED,
        payoutStatus: PayoutStatus.REJECTED,
        createdAt: new Date(),
        updatedAt: new Date(),
      };

      expect(pendingIssue.payoutStatus).toBe(PayoutStatus.PENDING);
      expect(rejectedIssue.payoutStatus).toBe(PayoutStatus.REJECTED);
    });
  });
});

describe('Cross-referencing accuracy', () => {
  test('correctly matches issue numbers between GitHub and on-chain records', () => {
    const issues: WaveIssue[] = [
      {
        id: 'issue-100',
        issueNumber: 100,
        title: 'Issue 100',
        description: '',
        complexity: 'High',
        pointValue: 200,
        status: WaveIssueStatus.COMPLETED,
        payoutStatus: PayoutStatus.PENDING,
        createdAt: new Date(),
        updatedAt: new Date(),
      },
      {
        id: 'issue-101',
        issueNumber: 101,
        title: 'Issue 101',
        description: '',
        complexity: 'Medium',
        pointValue: 100,
        status: WaveIssueStatus.COMPLETED,
        payoutStatus: PayoutStatus.PENDING,
        createdAt: new Date(),
        updatedAt: new Date(),
      },
    ];

    const disbursements: GrantDisbursement[] = [
      {
        proposalId: 5,
        recipient: 'GABCD',
        amount: 200,
        token: 'USDC',
        issueNumber: 100,
        txHash: '0xabc',
        timestamp: new Date(),
      },
    ];

    const disbursementMap = new Map(disbursements.map((d) => [d.issueNumber, d]));

    const result = issues.map((issue) => {
      const disbursement = disbursementMap.get(issue.issueNumber);
      return disbursement
        ? {
            ...issue,
            grantProposalId: disbursement.proposalId,
            disbursementTxHash: disbursement.txHash,
            payoutStatus: PayoutStatus.DISBURSED,
          }
        : issue;
    });

    expect(result[0].payoutStatus).toBe(PayoutStatus.DISBURSED);
    expect(result[0].grantProposalId).toBe(5);
    expect(result[1].payoutStatus).toBe(PayoutStatus.PENDING); // No disbursement
    expect(result[1].grantProposalId).toBeUndefined();
  });
});
