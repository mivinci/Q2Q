import assert from 'node:assert/strict';
import test from 'node:test';
import { expectedSchema, validateSchema } from '../scripts/check-schema.ts';

test('deployment schema gate accepts fresh migration definitions and harmless formatting', () => {
  const rows = expectedSchema();
  assert.doesNotThrow(() => validateSchema(rows));
  assert.doesNotThrow(() => validateSchema([...rows].reverse().map(row => ({ ...row, sql: `-- comment\n${row.sql}\n;` }))));
});

test('deployment schema gate rejects old, partial and incompatible schemas even with the new column', () => {
  const rows = expectedSchema();
  const change = (sql: string) => rows.map(row => row.name === 'nodes' ? { ...row, sql } : row);
  const nodes = rows.find(row => row.name === 'nodes')!.sql;
  for (const sql of [
    nodes.replace('advertise_url TEXT,', 'advertise_url TEXT NOT NULL,'),
    nodes.replace('  public_key TEXT,', '  websocket_url TEXT,\n  public_key TEXT,'),
    nodes.replace('BETWEEN 1 AND 8', 'BETWEEN 0 AND 8'),
    nodes.replace("service_type = 'punch'", "service_type = 'punch '"),
    nodes.replace('endpoints_json', 'old_endpoints'),
  ]) {
    assert.notEqual(sql, nodes);
    assert.throws(() => validateSchema(change(sql)), /schema differs/);
  }
  for (const row of rows) assert.throws(() => validateSchema(rows.filter(value => value !== row)), /schema differs/);
  assert.throws(() => validateSchema([...rows, { type:'trigger',name:'unexpected',tbl_name:'nodes',sql:'arbitrary unexecuted text' }]), /schema differs/);
});

test('deployment schema gate fails closed on malformed results', () => {
  for (const input of [null, {}, [], [null], [{ name:'nodes',sql:'secret' }]]) assert.throws(() => validateSchema(input));
  const rows = expectedSchema();
  assert.throws(() => validateSchema([...rows, rows[0]]), /Duplicate/);
});
