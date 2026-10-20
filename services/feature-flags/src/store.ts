/**
 * Feature Flag Service — Store
 *
 * Persists flag definitions in SQLite. Provides:
 *   - CRUD for flag definitions
 *   - In-memory cache with configurable TTL for fast reads
 *   - Change notification callbacks for the kill-switch SSE stream
 */

import Database from "better-sqlite3";
import pino from "pino";
import { FeatureFlag, FeatureName, TargetingRule } from "./types";

const logger = pino({ name: "feature-flags:store" });

export type FlagChangeCallback = (flag: FeatureFlag) => void;

export class FlagStore {
  private readonly db: Database.Database;
  private cache: Map<FeatureName, FeatureFlag> = new Map();
  private cacheLoadedAt = 0;
  private readonly cacheTtlMs: number;
  private changeCallbacks: FlagChangeCallback[] = [];

  constructor(
    dbPath: string,
    opts: { cacheTtlMs?: number } = {}
  ) {
    this.db = new Database(dbPath);
    this.cacheTtlMs = opts.cacheTtlMs ?? 5_000; // 5s default — fast enough for kill-switch
    this.migrate();
  }

  // ── Schema ───────────────────────────────────────────────────────────────

  private migrate(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS feature_flags (
        name        TEXT PRIMARY KEY,
        description TEXT NOT NULL DEFAULT '',
        enabled     INTEGER NOT NULL DEFAULT 0,
        rules_json  TEXT NOT NULL DEFAULT '[]',
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        updated_by  TEXT NOT NULL DEFAULT 'system'
      );
    `);
    logger.info("Feature flag schema migrated");
  }

  // ── Write ─────────────────────────────────────────────────────────────────

  upsert(flag: Omit<FeatureFlag, "createdAt" | "updatedAt"> & { createdAt?: string }): FeatureFlag {
    const now = new Date().toISOString();
    const existing = this.getByName(flag.name);
    const createdAt = flag.createdAt ?? existing?.createdAt ?? now;

    const full: FeatureFlag = {
      ...flag,
      createdAt,
      updatedAt: now,
    };

    this.db
      .prepare(
        `INSERT INTO feature_flags (name, description, enabled, rules_json, created_at, updated_at, updated_by)
         VALUES (@name, @description, @enabled, @rules_json, @created_at, @updated_at, @updated_by)
         ON CONFLICT(name) DO UPDATE SET
           description = excluded.description,
           enabled     = excluded.enabled,
           rules_json  = excluded.rules_json,
           updated_at  = excluded.updated_at,
           updated_by  = excluded.updated_by`
      )
      .run({
        name: full.name,
        description: full.description,
        enabled: full.enabled ? 1 : 0,
        rules_json: JSON.stringify(full.rules),
        created_at: full.createdAt,
        updated_at: full.updatedAt,
        updated_by: full.updatedBy,
      });

    this.invalidateCache();
    this.notifyChange(full);
    logger.info({ flag: full.name, enabled: full.enabled }, "Flag upserted");
    return full;
  }

  /**
   * Instant kill-switch: disable a flag without changing its rules.
   * This is the fast-path for incident response — no redeploy, propagates within
   * the cache TTL (default 5s) to all evaluations.
   */
  setEnabled(name: FeatureName, enabled: boolean, updatedBy: string): FeatureFlag {
    const flag = this.getByName(name);
    if (!flag) {
      throw new Error(`Feature flag '${name}' not found`);
    }
    return this.upsert({ ...flag, enabled, updatedBy });
  }

  delete(name: FeatureName): void {
    this.db.prepare(`DELETE FROM feature_flags WHERE name = ?`).run(name);
    this.cache.delete(name);
    logger.info({ flag: name }, "Flag deleted");
  }

  close(): void {
    this.db.close();
  }

  // ── Read ──────────────────────────────────────────────────────────────────

  getByName(name: FeatureName): FeatureFlag | null {
    const all = this.loadAll();
    return all.get(name) ?? null;
  }

  getAll(): FeatureFlag[] {
    return Array.from(this.loadAll().values());
  }

  // ── Change Notification (for kill-switch SSE) ─────────────────────────────

  onFlagChange(cb: FlagChangeCallback): void {
    this.changeCallbacks.push(cb);
  }

  // ── Internal ──────────────────────────────────────────────────────────────

  private loadAll(): Map<FeatureName, FeatureFlag> {
    const now = Date.now();
    if (now - this.cacheLoadedAt < this.cacheTtlMs && this.cache.size > 0) {
      return this.cache;
    }

    const rows = this.db
      .prepare(`SELECT * FROM feature_flags`)
      .all() as Array<{
        name: string;
        description: string;
        enabled: number;
        rules_json: string;
        created_at: string;
        updated_at: string;
        updated_by: string;
      }>;

    const newCache = new Map<FeatureName, FeatureFlag>();
    for (const row of rows) {
      newCache.set(row.name as FeatureName, {
        name: row.name as FeatureName,
        description: row.description,
        enabled: row.enabled === 1,
        rules: JSON.parse(row.rules_json) as TargetingRule[],
        createdAt: row.created_at,
        updatedAt: row.updated_at,
        updatedBy: row.updated_by,
      });
    }

    this.cache = newCache;
    this.cacheLoadedAt = now;
    return this.cache;
  }

  private invalidateCache(): void {
    this.cacheLoadedAt = 0;
  }

  private notifyChange(flag: FeatureFlag): void {
    for (const cb of this.changeCallbacks) {
      try {
        cb(flag);
      } catch (err) {
        logger.error({ err, flag: flag.name }, "Flag change callback error");
      }
    }
  }
}
