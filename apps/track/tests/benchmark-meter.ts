export function instrument(storage: DurableObjectStorage) {
  const sql = storage.sql, execute = sql.exec.bind(sql);
  const incarnation = crypto.randomUUID();
  const counts = {alarm_sets: 0, kv_gets: 0, kv_puts: 0, kv_deletes: 0, statements: 0, rows_read: 0, rows_written: 0, rpc: 0, alarms: 0};
  Object.defineProperty(sql, 'exec', {value: (...args: Parameters<SqlStorage['exec']>) => {
    const cursor = execute(...args);
    counts.statements++;
    let read = 0, written = 0;
    const record = () => {
      counts.rows_read += cursor.rowsRead - read; counts.rows_written += cursor.rowsWritten - written;
      read = cursor.rowsRead; written = cursor.rowsWritten;
    };
    record();
    const wrapped: typeof cursor = new Proxy(cursor, {get(target, name) {
      if (name === Symbol.iterator) return () => wrapped;
      const value = Reflect.get(target, name, target);
      if (typeof value !== 'function') return value;
      return (...parameters: unknown[]) => {try {return Reflect.apply(value, target, parameters);} finally {record();}};
    }});
    return wrapped;
  }});
  for (const [name, counter] of [['setAlarm', 'alarm_sets'], ['get', 'kv_gets'], ['put', 'kv_puts'], ['delete', 'kv_deletes']] as const) {
    const method = storage[name];
    Object.defineProperty(storage, name, {value: (...args: unknown[]) => {
      counts[counter]++; return Reflect.apply(method, storage, args);
    }});
  }
  return {counts, snapshot(reset = false) {
    // Observer reads are deliberately excluded from workload SQL accounting.
    const table = execute("SELECT name FROM sqlite_master WHERE type='table' AND name='outbox'").toArray().length;
    const pending = table ? execute<{n:number}>('SELECT count(*) AS n FROM outbox').one().n : 0;
    const result = {...counts, incarnation, pending, database_bytes: sql.databaseSize};
    if (reset) for (const key of Object.keys(counts) as (keyof typeof counts)[]) counts[key] = 0;
    return result;
  }};
}
export type Meter = ReturnType<typeof instrument>;
