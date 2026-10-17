// Opt-in live check. Uses synthetic data under a unique prefix and cleans up only its own resources.
import assert from 'node:assert/strict';
import { randomFillSync, randomUUID, createHash } from 'node:crypto';
import { AwsClient } from 'aws4fetch';
import { createS3Storage, signS3Part, s3ObjectUrl } from '../src/lib/server/s3-storage.ts';

const config = {
  endpoint: process.env.S3_ENDPOINT, region: process.env.S3_REGION, bucket: process.env.S3_BUCKET,
  accessKeyId: process.env.S3_ACCESS_KEY_ID, secretAccessKey: process.env.S3_SECRET_ACCESS_KEY,
  forcePathStyle: process.env.S3_FORCE_PATH_STYLE !== 'false'
};
if (Object.entries(config).some(([, value]) => value === undefined || value === '')) throw new Error('Configure S3_* environment variables first');
const storage = createS3Storage(config);
const client = new AwsClient({ accessKeyId: config.accessKeyId, secretAccessKey: config.secretAccessKey, region: config.region, service: 's3', retries: 0 });
const prefix = `q2q-integration/${randomUUID()}`;
const key = `${prefix}/encoded%3A/complete.bin`;
const cancelKey = `${prefix}/cancel.bin`;
const origin = 'http://localhost:5173';
const bytes = randomFillSync(Buffer.alloc(8 * 1024 * 1024 + 257));
const hash = (data) => createHash('sha256').update(data).digest('hex');
let created;
let cancelled;
let corsReady = true;
let failure;
const results = {};
try {
  created = await storage.createMultipartUpload(key, 'live-check');
  results.initialization = true;
  const receipts = [];
  for (let index = 0; index < 2; index++) {
    const part = bytes.subarray(index * 8 * 1024 * 1024, Math.min((index + 1) * 8 * 1024 * 1024, bytes.length));
    const grant = await signS3Part(config, key, created.uploadId, index + 1, part.length, 60);
    if (index === 1) {
      const rejected = await fetch(grant.url, { method: 'PUT', headers: grant.headers, body: part.subarray(0, part.length - 1) });
      assert.equal(rejected.status, 403, 'A changed signed Content-Length must be rejected');
      await rejected.body?.cancel();
      results.lengthConstraint = true;
    }
    const response = await fetch(grant.url, { method: 'PUT', headers: { ...grant.headers, Origin: origin }, body: part });
    assert(response.ok, `Part upload HTTP ${response.status}`);
    const etag = response.headers.get('etag')?.replace(/^"|"$/g, '');
    assert(etag, 'Missing part ETag');
    const exposed = (response.headers.get('access-control-expose-headers') ?? '').toLowerCase().split(',').map((value) => value.trim());
    corsReady &&= response.headers.get('access-control-allow-origin') === origin && (exposed.includes('etag') || exposed.includes('*'));
    receipts.push({ partNumber: index + 1, etag });
    await response.body?.cancel();
  }
  results.presignedParts = true;
  const object = await storage.resumeMultipartUpload(key, created.uploadId).complete(receipts);
  assert.equal(object.size, bytes.length);
  assert.equal(object.customMetadata?.uploadTask, 'live-check');
  const response = await client.fetch(s3ObjectUrl(config, key).toString());
  assert(response.ok, `Readback HTTP ${response.status}`);
  assert.equal(hash(Buffer.from(await response.arrayBuffer())), hash(bytes));
  results.completionAndByteIntegrity = true;
  cancelled = await storage.createMultipartUpload(cancelKey, 'live-cancel');
  const grant = await signS3Part(config, cancelKey, cancelled.uploadId, 1, 257, 60);
  const part = await fetch(grant.url, { method: 'PUT', headers: grant.headers, body: bytes.subarray(0, 257) });
  assert(part.ok, `Cancellation fixture HTTP ${part.status}`);
  await part.body?.cancel();
  await cancelled.abort();
  await cancelled.abort(); // Repeated cancellation must be harmless.
  assert.equal(await storage.head(cancelKey), null);
  results.cancellation = true;
  const expired = await signS3Part(config, cancelKey, cancelled.uploadId, 1, 257, 1);
  await new Promise((resolve) => setTimeout(resolve, 2100));
  const denied = await fetch(expired.url, { method: 'PUT', headers: expired.headers, body: bytes.subarray(0, 257) });
  assert.equal(denied.status, 403, 'An expired grant must be rejected');
  await denied.body?.cancel();
  results.expiration = true;
  results.browserCors = corsReady;
} catch (cause) { failure = cause; }
finally {
  const cleanup = await Promise.allSettled([
    created?.abort(), cancelled?.abort(), storage.delete(key), storage.delete(cancelKey)
  ]);
  results.cleanup = cleanup.every((result) => result.status === 'fulfilled');
  if (!results.cleanup) failure ??= new Error('Temporary resource cleanup failed');
}
console.log(JSON.stringify(results));
if (failure) { console.error(failure instanceof Error ? failure.message : 'S3 live check failed'); process.exitCode = 1; }
else if (!corsReady) { console.error('Direct S3 operations passed, but browser CORS must allow the origin and expose ETag.'); process.exitCode = 1; }
