import { DatabaseSync } from 'node:sqlite';
import { readFileSync, readdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

type Definition = { type: string; name: string; tbl_name: string; sql: string };
const directory = new URL('../migrations/', import.meta.url);
const tables = ['nodes', 'report_limits', 'punch_policy', 'punch_operation'];
// Schema only: do not query node data, credentials, or audit contents.
export const schemaQuery = `SELECT type,name,tbl_name,sql FROM sqlite_master WHERE tbl_name IN ('nodes','report_limits','punch_policy','punch_operation') AND sql IS NOT NULL ORDER BY type,name`;
const normalize = (sql: string) => sql.replace(/'(?:''|[^'])*'|"(?:""|[^"])*"|--[^\n]*|\/\*[\s\S]*?\*\/|\s+/g,
  token => token.startsWith('--') || token.startsWith('/*') || /^\s/.test(token) ? '' : token).replace(/;$/, '');

export function expectedSchema(): Definition[] {
  const db = new DatabaseSync(':memory:');
  try {
    for (const name of readdirSync(directory).filter(name => name.endsWith('.sql')).sort()) {
      db.exec(readFileSync(new URL(name, directory), 'utf8'));
    }
    return db.prepare(schemaQuery).all() as Definition[];
  } finally { db.close(); }
}

/** A new column alone is insufficient: reject old NOT NULL rules, old columns,
 * missing constraints/indexes/triggers, and partial hand-edited schemas as well.
 * Never execute SQL received from the remote database. */
export function validateSchema(input: unknown): void {
  if (!Array.isArray(input)) throw new Error('Invalid Registry schema response');
  const actual = new Map<string, string>();
  for (const row of input) {
    if (!row || typeof row !== 'object' || typeof row.type !== 'string' || typeof row.name !== 'string' ||
      typeof row.tbl_name !== 'string' || typeof row.sql !== 'string' || !tables.includes(row.tbl_name)) {
      throw new Error('Invalid Registry schema definition');
    }
    const key = `${row.type}:${row.name}`;
    if (actual.has(key)) throw new Error('Duplicate Registry schema definition');
    actual.set(key, normalize(row.sql));
  }
  const expected = expectedSchema();
  if (actual.size !== expected.length || expected.some(row => actual.get(`${row.type}:${row.name}`) !== normalize(row.sql))) {
    throw new Error('Registry schema differs from the current initial schema; coordinate the scoped reset before deploying. No migration or reset was performed.');
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv.length !== 3 || process.argv[2] !== '--remote') throw new Error('Expected --remote');
  const output = execFileSync('npx', ['wrangler', 'd1', 'execute', 'DB', '--remote', '--json', '--command', schemaQuery],
    { cwd: new URL('../', import.meta.url), encoding: 'utf8', maxBuffer: 1024 * 1024 });
  const result: unknown = JSON.parse(output);
  if (!Array.isArray(result) || result.length !== 1 || result[0]?.success !== true) throw new Error('Registry schema query failed');
  validateSchema(result[0].results);
  console.log('Registry schema matches the current initial schema (read-only check).');
}
