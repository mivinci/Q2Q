import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fileSource, StorageError } from '../src/index.ts';

const code = (expected: string) => (error: unknown) =>
  error instanceof StorageError && error.code === expected;

function delayedFile() {
  let release!: (bytes: ArrayBuffer) => void;
  const pending = new Promise<ArrayBuffer>(resolve => { release = resolve; });
  const file = new File(['abcd'], 'data.bin');
  file.slice = () => ({ arrayBuffer: () => pending }) as Blob;
  return { file, release };
}

test('import and source creation do not touch DOM globals or read the file', async () => {
  const file = new File(['abcdef'], 'data.bin');
  let reads = 0;
  const slice = file.slice.bind(file);
  file.slice = (...args) => { reads++; return slice(...args); };
  const source = fileSource(file);
  assert.deepEqual(source.info, { name: 'data.bin', size: 6n });
  assert.ok(Object.isFrozen(source.info));
  const reader = await source.open();
  assert.equal(reads, 0);
  assert.equal(new TextDecoder().decode(await reader.read(2n, 3)), 'cde');
  assert.equal(reads, 1);
  await reader.close();
});

test('parallel out-of-order reads and reader leases are independent', async () => {
  const source = fileSource(new File(['abcdef'], 'x'));
  const a = await source.open();
  const b = await source.open();
  const result = await Promise.all([a.read(4n, 2), b.read(0n, 2), a.read(2n, 2)]);
  assert.deepEqual(result.map(bytes => new TextDecoder().decode(bytes)), ['ef', 'ab', 'cd']);
  await a.close();
  await assert.rejects(a.read(0n, 1), code('INVALID_STATE'));
  assert.equal((await b.read(0n, 1))[0], 97);
  await b.close();
});

test('rejects negative, unsafe, out-of-bounds and over-budget ranges', async () => {
  const reader = await fileSource(new File(['abcd'], 'x'), { maxReadBytes: 2 }).open();
  for (const [offset, length] of [
    [-1n, 1], [0n, -1], [0n, 1.5], [0n, NaN], [0n, Infinity],
    [0n, 3], [3n, 2], [5n, 0], [2n ** 60n, 1]
  ] as const) {
    await assert.rejects(reader.read(offset, length), code('INVALID_RANGE'));
  }
  assert.equal((await reader.read(4n, 0)).length, 0);
  await reader.close();
});

test('empty files support an empty range', async () => {
  const reader = await fileSource(new File([], 'empty')).open();
  assert.equal((await reader.read(0n, 0)).length, 0);
  await assert.rejects(reader.read(0n, 1), code('INVALID_RANGE'));
  await reader.close();
});

test('validates read budget and rejects unsafe host file sizes', () => {
  for (const maxReadBytes of [0, -1, Infinity, 1.5]) {
    assert.throws(() => fileSource(new File([], 'x'), { maxReadBytes }), code('INVALID_RANGE'));
  }
  const file = new File([], 'x');
  Object.defineProperty(file, 'size', { value: Number.MAX_SAFE_INTEGER + 1 });
  assert.throws(() => fileSource(file), code('UNSUPPORTED_CAPABILITY'));
});

test('pre-aborted reads never start host IO', async () => {
  const { file } = delayedFile();
  file.slice = () => { throw new Error('must not read'); };
  const reader = await fileSource(file).open();
  await assert.rejects(reader.read(0n, 4, { signal: AbortSignal.abort() }), code('TASK_CANCELLED'));
  await reader.close();
});

test('abort during host IO suppresses late data without poisoning the lease', async () => {
  const { file, release } = delayedFile();
  const reader = await fileSource(file).open();
  const controller = new AbortController();
  const read = reader.read(0n, 4, { signal: controller.signal });
  const rejected = assert.rejects(read, code('TASK_CANCELLED'));
  controller.abort();
  release(new Uint8Array([1, 2, 3, 4]).buffer);
  await rejected;
  assert.equal((await reader.read(0n, 4)).length, 4);
  await reader.close();
});

test('close fences late results and waits for actual host read completion', async () => {
  const { file, release } = delayedFile();
  const reader = await fileSource(file).open();
  const rejected = assert.rejects(reader.read(0n, 4), code('TASK_CANCELLED'));
  const closing = reader.close();
  assert.equal(reader.close(), closing);
  let done = false;
  void closing.then(() => { done = true; });
  await Promise.resolve();
  assert.equal(done, false);
  await assert.rejects(reader.read(0n, 1), code('INVALID_STATE'));
  release(new ArrayBuffer(4));
  await Promise.all([closing, rejected]);
  assert.equal(done, true);
});

test('short reads are rejected as changed content', async () => {
  const file = new File(['abcd'], 'x');
  file.slice = () => new Blob(['a']);
  const reader = await fileSource(file).open();
  await assert.rejects(reader.read(0n, 4), code('SOURCE_CHANGED'));
  await reader.close();
});

test('host errors are mapped without leaking exception messages', async () => {
  for (const [name, expected] of [
    ['NotAllowedError', 'PERMISSION_REQUIRED'], ['NotReadableError', 'SOURCE_UNAVAILABLE']
  ]) {
    const file = new File(['a'], 'x');
    file.slice = () => { throw new DOMException('/private/secret/file', name); };
    const reader = await fileSource(file).open();
    await assert.rejects(reader.read(0n, 1), error => {
      assert.ok(code(expected)(error));
      assert.equal((error as Error).message, expected);
      return true;
    });
    await reader.close();
  }
});
