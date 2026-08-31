import { describe, it, expect } from "vitest";

/**
 * Changelog Generation Tests (Issue #656)
 *
 * Test suite for automating CHANGELOG.md updates from conventional commit
 * messages and PR labels, correlated with contract version/migration bumps.
 */

interface ConventionalCommit {
  type: "feat" | "fix" | "docs" | "style" | "refactor" | "test" | "chore" | "perf";
  scope?: string;
  subject: string;
  body?: string;
  breaking: boolean;
  hash: string;
}

interface ContractVersionBump {
  contract: string;
  oldVersion: string;
  newVersion: string;
  migrationFile?: string;
}

interface ChangelogEntry {
  version: string;
  date: string;
  contracts: Map<string, string[]>;
  breaking: string[];
  authors: string[];
}

class ConventionalCommitParser {
  static parse(message: string): Partial<ConventionalCommit> {
    const match = message.match(
      /^(\w+)(?:\(([^)]*)\))?:\s(.+?)(?:\n\n(.*))?$/s
    );

    if (!match) {
      return {};
    }

    const [, type, scope, subject, body] = match;
    const breaking = /^BREAKING CHANGE:/.test(body || "");

    return {
      type: type as ConventionalCommit["type"],
      scope,
      subject,
      body,
      breaking,
    };
  }

  static isSignificant(commit: Partial<ConventionalCommit>): boolean {
    // Filter out test-only and pure documentation changes
    const insignificantTypes = ["test", "docs", "style", "chore"];
    return (
      commit.type &&
      !insignificantTypes.includes(commit.type as string) &&
      !commit.subject?.includes("test only")
    );
  }
}

class ChangelogGenerator {
  static generateEntry(
    version: string,
    date: string,
    commits: ConventionalCommit[],
    versionBumps: ContractVersionBump[],
    authors: string[]
  ): ChangelogEntry {
    const entry: ChangelogEntry = {
      version,
      date,
      contracts: new Map(),
      breaking: [],
      authors: [...new Set(authors)],
    };

    // Group commits by scope (often correlates to contracts)
    for (const commit of commits) {
      if (!ConventionalCommitParser.isSignificant(commit)) {
        continue;
      }

      const scope = commit.scope || "general";
      if (!entry.contracts.has(scope)) {
        entry.contracts.set(scope, []);
      }

      const message = `${commit.type}: ${commit.subject}`;
      entry.contracts.get(scope)!.push(message);

      if (commit.breaking) {
        entry.breaking.push(`${scope}: ${commit.subject}`);
      }
    }

    // Associate version bumps with contracts
    for (const bump of versionBumps) {
      if (!entry.contracts.has(bump.contract)) {
        entry.contracts.set(bump.contract, []);
      }
      const versionMsg = `bumped to ${bump.newVersion}${
        bump.migrationFile ? ` (migration: ${bump.migrationFile})` : ""
      }`;
      entry.contracts.get(bump.contract)!.unshift(`version: ${versionMsg}`);
    }

    return entry;
  }

  static formatMarkdown(entry: ChangelogEntry): string {
    let markdown = `## [${entry.version}] - ${entry.date}\n\n`;

    if (entry.breaking.length > 0) {
      markdown += `### 🚨 Breaking Changes\n`;
      for (const breaking of entry.breaking) {
        markdown += `- ${breaking}\n`;
      }
      markdown += "\n";
    }

    for (const [contract, messages] of entry.contracts) {
      if (messages.length === 0) continue;

      markdown += `### ${contract}\n`;
      for (const message of messages) {
        markdown += `- ${message}\n`;
      }
      markdown += "\n";
    }

    return markdown;
  }
}

describe("Changelog Generation", () => {
  describe("Conventional Commit Parser", () => {
    it("should parse simple feat commit", () => {
      const message = "feat: add new invoice type support";
      const commit = ConventionalCommitParser.parse(message);

      expect(commit.type).toBe("feat");
      expect(commit.scope).toBeUndefined();
      expect(commit.subject).toBe("add new invoice type support");
      expect(commit.breaking).toBe(false);
    });

    it("should parse commit with scope", () => {
      const message = "fix(marketplace): handle edge case in listing validation";
      const commit = ConventionalCommitParser.parse(message);

      expect(commit.type).toBe("fix");
      expect(commit.scope).toBe("marketplace");
      expect(commit.subject).toBe("handle edge case in listing validation");
    });

    it("should detect breaking changes", () => {
      const message = `feat(access_control)!: change authorization model

BREAKING CHANGE: Old auth API no longer supported`;
      const commit = ConventionalCommitParser.parse(message);

      expect(commit.breaking).toBe(true);
      expect(commit.type).toBe("feat");
    });

    it("should return empty object for invalid format", () => {
      const message = "This is not a conventional commit";
      const commit = ConventionalCommitParser.parse(message);

      expect(Object.keys(commit).length).toBe(0);
    });

    it("should identify insignificant commits (test only, docs, style)", () => {
      const testOnlyCommit = ConventionalCommitParser.parse(
        "test: add coverage for marketplace"
      );
      expect(ConventionalCommitParser.isSignificant(testOnlyCommit)).toBe(false);

      const docsCommit = ConventionalCommitParser.parse("docs: update README");
      expect(ConventionalCommitParser.isSignificant(docsCommit)).toBe(false);

      const styleCommit = ConventionalCommitParser.parse("style: format code");
      expect(ConventionalCommitParser.isSignificant(styleCommit)).toBe(false);

      const featCommit = ConventionalCommitParser.parse("feat: new feature");
      expect(ConventionalCommitParser.isSignificant(featCommit)).toBe(true);

      const fixCommit = ConventionalCommitParser.parse("fix: bug fix");
      expect(ConventionalCommitParser.isSignificant(fixCommit)).toBe(true);
    });
  });

  describe("Changelog Entry Generation", () => {
    it("should generate changelog entry from commits and version bumps", () => {
      const commits: ConventionalCommit[] = [
        {
          type: "feat",
          scope: "marketplace",
          subject: "add bulk listing support",
          breaking: false,
          hash: "abc123",
        },
        {
          type: "fix",
          scope: "financing_pool",
          subject: "fix rounding error in interest calculation",
          breaking: false,
          hash: "def456",
        },
        {
          type: "test",
          scope: "marketplace",
          subject: "add edge case tests",
          breaking: false,
          hash: "ghi789",
        },
      ];

      const versionBumps: ContractVersionBump[] = [
        {
          contract: "marketplace",
          oldVersion: "1.0.0",
          newVersion: "1.1.0",
        },
        {
          contract: "financing_pool",
          oldVersion: "2.1.0",
          newVersion: "2.1.1",
          migrationFile: "0042_fix_rounding.sql",
        },
      ];

      const entry = ChangelogGenerator.generateEntry(
        "1.5.0",
        "2025-08-30",
        commits,
        versionBumps,
        ["alice@example.com", "bob@example.com"]
      );

      expect(entry.version).toBe("1.5.0");
      expect(entry.date).toBe("2025-08-30");
      expect(entry.contracts.has("marketplace")).toBe(true);
      expect(entry.contracts.has("financing_pool")).toBe(true);
      expect(entry.breaking).toHaveLength(0);
      expect(entry.authors).toContain("alice@example.com");
      expect(entry.authors).toContain("bob@example.com");
    });

    it("should filter out insignificant commits from changelog", () => {
      const commits: ConventionalCommit[] = [
        {
          type: "feat",
          scope: "marketplace",
          subject: "add new feature",
          breaking: false,
          hash: "abc123",
        },
        {
          type: "test",
          scope: "marketplace",
          subject: "add tests",
          breaking: false,
          hash: "def456",
        },
      ];

      const entry = ChangelogGenerator.generateEntry(
        "1.0.0",
        "2025-08-30",
        commits,
        [],
        []
      );

      const messages = entry.contracts.get("marketplace") || [];
      expect(messages).toContain("feat: add new feature");
      expect(messages.some((m) => m.includes("add tests"))).toBe(false);
    });

    it("should track breaking changes separately", () => {
      const commits: ConventionalCommit[] = [
        {
          type: "feat",
          scope: "access_control",
          subject: "change authorization model",
          breaking: true,
          hash: "xyz789",
        },
        {
          type: "fix",
          scope: "marketplace",
          subject: "fix minor issue",
          breaking: false,
          hash: "abc123",
        },
      ];

      const entry = ChangelogGenerator.generateEntry(
        "2.0.0",
        "2025-08-30",
        commits,
        [],
        []
      );

      expect(entry.breaking).toHaveLength(1);
      expect(entry.breaking[0]).toContain("access_control");
      expect(entry.breaking[0]).toContain("change authorization model");
    });
  });

  describe("Markdown Formatting", () => {
    it("should format changelog entry as markdown", () => {
      const entry: ChangelogEntry = {
        version: "1.0.0",
        date: "2025-08-30",
        contracts: new Map([
          ["marketplace", ["feat: add bulk listing support"]],
          ["financing_pool", ["fix: correct interest calculation", "version: bumped to 2.1.1"]],
        ]),
        breaking: ["access_control: new auth model"],
        authors: ["author@example.com"],
      };

      const markdown = ChangelogGenerator.formatMarkdown(entry);

      expect(markdown).toContain("## [1.0.0] - 2025-08-30");
      expect(markdown).toContain("### 🚨 Breaking Changes");
      expect(markdown).toContain("- access_control: new auth model");
      expect(markdown).toContain("### marketplace");
      expect(markdown).toContain("- feat: add bulk listing support");
      expect(markdown).toContain("### financing_pool");
      expect(markdown).toContain("- fix: correct interest calculation");
    });

    it("should omit breaking changes section if none exist", () => {
      const entry: ChangelogEntry = {
        version: "1.0.0",
        date: "2025-08-30",
        contracts: new Map([["marketplace", ["feat: new feature"]]]),
        breaking: [],
        authors: [],
      };

      const markdown = ChangelogGenerator.formatMarkdown(entry);

      expect(markdown).not.toContain("Breaking Changes");
      expect(markdown).toContain("### marketplace");
    });

    it("should omit contracts with no changes", () => {
      const entry: ChangelogEntry = {
        version: "1.0.0",
        date: "2025-08-30",
        contracts: new Map([
          ["marketplace", ["feat: new feature"]],
          ["empty_contract", []],
        ]),
        breaking: [],
        authors: [],
      };

      const markdown = ChangelogGenerator.formatMarkdown(entry);

      expect(markdown).toContain("### marketplace");
      expect(markdown).not.toContain("### empty_contract");
    });
  });

  describe("Integration: Full Changelog Generation", () => {
    it("should generate complete changelog from multiple PRs", () => {
      const allCommits: ConventionalCommit[] = [
        {
          type: "feat",
          scope: "marketplace",
          subject: "add bulk listing API",
          breaking: false,
          hash: "commit1",
        },
        {
          type: "feat",
          scope: "invoice_nft",
          subject: "support custom metadata",
          breaking: false,
          hash: "commit2",
        },
        {
          type: "fix",
          scope: "marketplace",
          subject: "prevent duplicate listings",
          breaking: false,
          hash: "commit3",
        },
        {
          type: "perf",
          scope: "financing_pool",
          subject: "optimize rate calculation",
          breaking: false,
          hash: "commit4",
        },
        {
          type: "test",
          scope: "general",
          subject: "add integration tests",
          breaking: false,
          hash: "commit5",
        },
      ];

      const versionBumps: ContractVersionBump[] = [
        {
          contract: "marketplace",
          oldVersion: "1.0.0",
          newVersion: "1.1.0",
        },
        {
          contract: "invoice_nft",
          oldVersion: "1.0.0",
          newVersion: "1.1.0",
        },
        {
          contract: "financing_pool",
          oldVersion: "2.0.0",
          newVersion: "2.0.1",
        },
      ];

      const entry = ChangelogGenerator.generateEntry(
        "0.2.0",
        "2025-08-30",
        allCommits,
        versionBumps,
        ["alice@example.com", "charlie@example.com"]
      );

      expect(entry.contracts.size).toBeGreaterThan(0);
      expect(entry.contracts.has("marketplace")).toBe(true);
      expect(entry.contracts.has("invoice_nft")).toBe(true);
      expect(entry.contracts.has("financing_pool")).toBe(true);

      const markdown = ChangelogGenerator.formatMarkdown(entry);
      expect(markdown).toContain("[0.2.0]");
      expect(markdown).toContain("marketplace");
      expect(markdown).toContain("invoice_nft");
      expect(markdown).toContain("financing_pool");
    });
  });
});
