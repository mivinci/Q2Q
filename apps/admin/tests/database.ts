import { DatabaseSync } from 'node:sqlite';
import { readFileSync, readdirSync } from 'node:fs';

export function database(beforeMigration?: (name: string, sql: DatabaseSync) => void) {
  const sql = new DatabaseSync(':memory:');
  sql.exec('PRAGMA foreign_keys = ON');
  const migrations = new URL('../../platform/migrations/', import.meta.url);
  for (const name of readdirSync(migrations).sort()) if (name.endsWith('.sql')) { beforeMigration?.(name, sql); sql.exec(readFileSync(new URL(name, migrations), 'utf8')); }
  sql.exec(`INSERT INTO user (id,name,email,createdAt,updatedAt) VALUES ('user','Owner','owner@example.test',0,0);
    INSERT INTO project (id,name,createdAt) VALUES ('project','Example',0);`);
  let tail = Promise.resolve();
  const db = {
    prepare(query: string) {
      let args: (string | number | null)[] = [];
      return {
        bind(...values: (string | number | null)[]) { args = values; return this; },
        async run() { return this.all(); },
        async first() { return sql.prepare(query).get(...args) ?? null; },
        async all() {
          const before = Number(sql.prepare('SELECT total_changes() AS count').get()!.count);
          const results = sql.prepare(query).all(...args);
          const after = Number(sql.prepare('SELECT total_changes() AS count').get()!.count);
          return { results, success: true, meta: { changes: after - before } };
        }
      };
    },
    batch(statements: { all: () => Promise<unknown> }[]) {
      const job = tail.then(async () => {
        sql.exec('BEGIN');
        try {
          const results = [];
          for (const statement of statements) results.push(await statement.all());
          sql.exec('COMMIT'); return results;
        } catch (cause) { sql.exec('ROLLBACK'); throw cause; }
      });
      tail = job.then(() => {}, () => {});
      return job;
    }
  } as unknown as D1Database;
  return { sql, db };
}
