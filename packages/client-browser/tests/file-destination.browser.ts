import { fileDestination, StorageError } from '../src/index.ts';
import type { DestinationWriter } from '../src/index.ts';

function assert(value: unknown, message = 'assertion failed'): asserts value {
  if (!value) throw new Error(message);
}
async function rejects(promise: Promise<unknown>, expected: string) {
  try { await promise; } catch (error) {
    assert(error instanceof StorageError && error.code === expected,
      `expected ${expected}, got ${error instanceof Error ? error.message : String(error)}`);
    return;
  }
  throw new Error(`expected ${expected}`);
}
const bytes = (value: string) => new TextEncoder().encode(value);
async function text(handle: FileSystemFileHandle) { return (await handle.getFile()).text(); }

async function removeFixtureFile(root: Pick<FileSystemDirectoryHandle, 'removeEntry'>, name: string) {
  // Chromium can acknowledge abort before the browser process releases its native
  // file lock. Only teardown tolerates that transient error; SDK operations and
  // immediate reopen assertions in each case must still succeed without retries.
  const deadline = performance.now() + 2000;
  for (;;) {
    try { await root.removeEntry(name); return; }
    catch (error) {
      if (!(error instanceof DOMException) || error.name !== 'NoModificationAllowedError'
        || performance.now() >= deadline) throw error;
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  }
}

async function fixture(run: (handle: FileSystemFileHandle) => Promise<void>) {
  const root = await navigator.storage.getDirectory().catch(error => {throw new Error(`fixture root: ${error.name}`);});
  const name = `q2q-test-${crypto.randomUUID()}`;
  const handle = await root.getFileHandle(name, { create: true }).catch(error => {throw new Error(`fixture create: ${error.name}`);});
  try { await run(handle); }
  catch (error) {
    // A leaked native stream can also make cleanup fail; retain the first failure.
    try { await removeFixtureFile(root, name); } catch { /* preserve the test failure */ }
    throw error;
  }
  try { await removeFixtureFile(root, name); }
  catch (error) { throw new Error(`fixture cleanup: ${error instanceof Error ? error.name : 'unknown'}`); }
}
async function seed(handle: FileSystemFileHandle, value: string) {
  const stream = await handle.createWritable();
  await stream.write(bytes(value));
  await stream.close();
}

export const cases: Record<string, () => Promise<void>> = {
  'fixture cleanup retries only transient native lock errors and remains bounded': async () => {
    let attempts = 0;
    await removeFixtureFile({ async removeEntry() {
      if (++attempts < 3) throw new DOMException('native lock pending', 'NoModificationAllowedError');
    } }, 'temporary.bin');
    assert(attempts === 3);
    for (const name of ['NotAllowedError', 'NotFoundError', 'NoModificationAllowedError']) {
      const failure = new DOMException('cleanup failure', name);
      attempts = 0;
      try {
        await removeFixtureFile({ async removeEntry() { attempts++; throw failure; } }, 'temporary.bin');
      } catch (error) {
        assert(error === failure);
        assert(name === 'NoModificationAllowedError' ? attempts > 1 : attempts === 1);
        continue;
      }
      throw new Error('cleanup failure must not be swallowed');
    }
  },
  'out-of-order writes stay invisible until commit; post-commit writes survive finish': async () => fixture(async handle => {
    const destination = await fileDestination(handle);
    const writer = await destination.open({ mode: 'create', size: 6n });
    await Promise.all([writer.write(3n, bytes('def')), writer.write(0n, bytes('abc'))]);
    assert(await text(handle) === '');
    await writer.commit();
    assert(await text(handle) === 'abcdef');
    await writer.write(1n, bytes('XY'));
    assert(await text(handle) === 'abcdef');
    const result = await writer.finish();
    assert(result.kind === 'staged' && result.needsExport && result.targetId === destination.targetId);
    assert(await text(handle) === 'aXYdef');
    assert(await writer.finish() === result);
    await writer.abort();
  }),
  'create refuses existing content; overwrite is explicit and abort preserves original': async () => fixture(async handle => {
    await seed(handle, 'original');
    await rejects((await fileDestination(handle)).open({ mode: 'create', size: 3n }), 'DESTINATION_EXISTS');
    const writer = await (await fileDestination(handle, { overwrite: true })).open({ mode: 'create', size: 3n });
    await writer.write(0n, bytes('new'));
    await writer.abort();
    assert(await text(handle) === 'original');
  }),
  'resume preserves untouched bytes and rejects a target longer than expected': async () => fixture(async handle => {
    await seed(handle, 'abcdef');
    const destination = await fileDestination(handle);
    await rejects(destination.open({ mode: 'resume', size: 5n }), 'CHECKPOINT_MISMATCH');
    const writer = await destination.open({ mode: 'resume', size: 6n });
    await writer.write(3n, bytes('XYZ'));
    await writer.finish();
    assert(await text(handle) === 'abcXYZ');
  }),
  'different handles and independently-created descriptors share one lease across commits': async () => fixture(async handle => {
    const root = await navigator.storage.getDirectory();
    const alias = await root.getFileHandle(handle.name);
    const [a, b] = await Promise.all([fileDestination(handle), fileDestination(alias)]);
    assert(a.targetId === b.targetId);
    const writer = await a.open({ mode: 'create', size: 4n });
    await rejects(b.open({ mode: 'create', size: 4n }), 'DESTINATION_BUSY');
    await writer.write(0n, bytes('abcd'));
    await writer.commit();
    await rejects(b.open({ mode: 'resume', size: 4n }), 'DESTINATION_BUSY');
    await writer.abort();
    const next = await b.open({ mode: 'resume', size: 4n });
    await next.abort();
  }),
  'different files can be written concurrently': async () => fixture(async a => fixture(async b => {
    const first = await (await fileDestination(a)).open({ mode: 'create', size: 1n });
    const second = await (await fileDestination(b)).open({ mode: 'create', size: 1n });
    await Promise.all([first.write(0n, bytes('a')), second.write(0n, bytes('b'))]);
    await Promise.all([first.finish(), second.finish()]);
    assert(await text(a) === 'a' && await text(b) === 'b');
  })),
  'write copies caller buffers and bounds pending bytes without poisoning the writer': async () => fixture(async handle => {
    const writer = await (await fileDestination(handle, { maxPendingBytes: 4 })).open({ mode: 'create', size: 4n });
    const input = bytes('abcd');
    const pending = writer.write(0n, input);
    input.fill(0);
    await rejects(writer.write(0n, bytes('x')), 'RESOURCE_LIMIT');
    await pending;
    await writer.finish();
    assert(await text(handle) === 'abcd');
  }),
  'invalid ranges and sizes do not change the file': async () => fixture(async handle => {
    const destination = await fileDestination(handle);
    await rejects(destination.open({ mode: 'create', size: -1n }), 'INVALID_RANGE');
    await rejects(destination.open({ mode: 'create', size: 2n ** 60n }), 'UNSUPPORTED_CAPABILITY');
    const writer = await destination.open({ mode: 'create', size: 4n });
    for (const offset of [-1n, 4n, 2n ** 60n]) await rejects(writer.write(offset, bytes('x')), 'INVALID_RANGE');
    await writer.write(0n, bytes('abcd'));
    await writer.finish();
    assert(await text(handle) === 'abcd');
  }),
  'abort fences queued writes and releases target only after cleanup': async () => fixture(async handle => {
    const destination = await fileDestination(handle);
    const writer = await destination.open({ mode: 'create', size: 4n });
    const pending = rejects(writer.write(0n, bytes('abcd')), 'TASK_CANCELLED');
    const abort = writer.abort();
    assert(writer.abort() === abort);
    await Promise.all([pending, abort]);
    assert(await text(handle) === '');
    await rejects(writer.write(0n, bytes('a')), 'INVALID_STATE');
    const next = await destination.open({ mode: 'create', size: 4n });
    await next.abort();
  }),
  'abort discards writes since the last successful commit': async () => fixture(async handle => {
    const writer = await (await fileDestination(handle)).open({ mode: 'create', size: 4n });
    await writer.write(0n, bytes('abcd'));
    await writer.commit();
    await writer.write(0n, bytes('xxxx'));
    await writer.abort();
    assert(await text(handle) === 'abcd');
  }),
  'open cancellation does not truncate content or retain the lease': async () => fixture(async handle => {
    const destination = await fileDestination(handle);
    await rejects(destination.open({ mode: 'create', size: 1n, signal: AbortSignal.abort() }), 'TASK_CANCELLED');
    const original = handle.createWritable.bind(handle);
    const controller = new AbortController();
    handle.createWritable = async options => {
      const stream = await original(options);
      controller.abort();
      return stream;
    };
    await rejects(destination.open({ mode: 'create', size: 1n, signal: controller.signal }), 'TASK_CANCELLED');
    handle.createWritable = original;
    const writer = await destination.open({ mode: 'create', size: 1n });
    await writer.abort();
  }),
  'abort waits for in-flight host writes and prevents queued commit': async () => fixture(async handle => {
    const destination = await fileDestination(handle);
    const original = handle.createWritable.bind(handle);
    let release!: () => void;
    let entered!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const started = new Promise<void>(resolve => { entered = resolve; });
    handle.createWritable = async options => {
      const stream = await original(options);
      const write = stream.write.bind(stream);
      stream.write = async data => { entered(); await gate; await write(data); };
      return stream;
    };
    const writer = await destination.open({ mode: 'create', size: 4n });
    const writing = rejects(writer.write(0n, bytes('abcd')), 'TASK_CANCELLED');
    await started;
    const committing = rejects(writer.commit(), 'TASK_CANCELLED');
    let done = false;
    const aborting = writer.abort().then(() => { done = true; });
    await Promise.resolve();
    assert(!done);
    await rejects(destination.open({ mode: 'create', size: 4n }), 'DESTINATION_BUSY');
    release();
    await Promise.all([writing, committing, aborting]);
    assert(await text(handle) === '');
    handle.createWritable = original;
  }),
  'abort during a non-interruptible close waits for it and does not report completion': async () => fixture(async handle => {
    const destination = await fileDestination(handle);
    const original = handle.createWritable.bind(handle);
    let release!: () => void;
    let entered!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const started = new Promise<void>(resolve => { entered = resolve; });
    handle.createWritable = async options => {
      const stream = await original(options);
      const close = stream.close.bind(stream);
      stream.close = async () => { entered(); await gate; await close(); };
      return stream;
    };
    const writer = await destination.open({ mode: 'create', size: 4n });
    await writer.write(0n, bytes('abcd'));
    const finishing = rejects(writer.finish(), 'TASK_CANCELLED');
    await started;
    const aborting = writer.abort();
    await rejects(destination.open({ mode: 'create', size: 4n }), 'DESTINATION_BUSY');
    release();
    await Promise.all([finishing, aborting]);
    // Close had already started, so its host commit may legitimately survive.
    assert(await text(handle) === 'abcd');
    handle.createWritable = original;
    const next = await destination.open({ mode: 'resume', size: 4n });
    await next.abort();
  }),
  'host quota failure is terminal, sanitized, and releases the target': async () => fixture(async handle => {
    const destination = await fileDestination(handle);
    const original = handle.createWritable.bind(handle);
    handle.createWritable = async options => {
      const stream = await original(options);
      stream.write = async () => { throw new DOMException('/private/path', 'QuotaExceededError'); };
      return stream;
    };
    const writer = await destination.open({ mode: 'create', size: 4n });
    await rejects(writer.write(0n, bytes('abcd')), 'STORAGE_FULL');
    await rejects(writer.commit(), 'STORAGE_FULL');
    await writer.abort();
    handle.createWritable = original;
    const next = await destination.open({ mode: 'create', size: 4n });
    await next.abort();
    assert(await text(handle) === '');
  }),
  'failed close does not report a saved file and releases the lease': async () => fixture(async handle => {
    const destination = await fileDestination(handle);
    const original = handle.createWritable.bind(handle);
    handle.createWritable = async options => {
      const stream = await original(options);
      stream.close = async () => { throw new DOMException('private text', 'NotAllowedError'); };
      return stream;
    };
    let stage = 'open';
    try {
      const writer = await destination.open({ mode: 'create', size: 1n });
      stage = 'write'; await writer.write(0n, bytes('x'));
      stage = 'finish'; await rejects(writer.finish(), 'PERMISSION_REQUIRED');
      handle.createWritable = original;
      stage = 'reopen'; const next = await destination.open({ mode: 'create', size: 1n });
      stage = 'abort'; await next.abort();
    } catch (error) {
      throw new Error(`failed-close ${stage}: ${error instanceof Error ? error.message : 'unknown'}`);
    } finally { handle.createWritable = original; }
  })
};

let held: DestinationWriter | undefined;
export async function holdFile(name: string) {
  const root = await navigator.storage.getDirectory();
  const handle = await root.getFileHandle(name, { create: true });
  const destination = await fileDestination(handle);
  held = await destination.open({ mode: 'resume', size: 4n });
  await held.write(0n, bytes('abcd'));
  await held.commit();
  return destination.targetId;
}
export async function contendFile(name: string) {
  const root = await navigator.storage.getDirectory();
  const destination = await fileDestination(await root.getFileHandle(name));
  await rejects(destination.open({ mode: 'resume', size: 4n }), 'DESTINATION_BUSY');
  return destination.targetId;
}
export async function releaseFile() { await held?.abort(); held = undefined; }
export async function reopenFile(name: string) {
  const root = await navigator.storage.getDirectory();
  const handle = await root.getFileHandle(name);
  const destination = await fileDestination(handle);
  const writer = await destination.open({ mode: 'resume', size: 4n });
  await writer.abort();
  assert(await text(handle) === 'abcd');
  return destination.targetId;
}
export async function removeFile(name: string) { await removeFixtureFile(await navigator.storage.getDirectory(), name); }
