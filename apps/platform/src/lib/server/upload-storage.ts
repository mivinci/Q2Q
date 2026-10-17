import { dev } from '$app/env';
import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import type { PartGrant } from '#lib/upload.js';
import { createS3Storage, signS3Part, type S3Config, type UploadStorage } from './s3-storage.js';

export function uploadTransport(): 'local' | 's3' | null {
  // Local proxying must never silently become the production data path.
  if (dev && env.STORAGE && env.UPLOAD_LOCAL_PROXY === 'true') return 'local';
  return env.S3_ENDPOINT && env.S3_REGION && env.S3_BUCKET && env.S3_ACCESS_KEY_ID && env.S3_SECRET_ACCESS_KEY ? 's3' : null;
}

export function storageBucket(): R2Bucket {
  if (!env.STORAGE) error(503, 'storageUnavailable');
  return env.STORAGE;
}

export function storageIdentity(mode = uploadTransport()): string {
  if (mode !== 's3') return 'local-r2';
  const config = s3Config();
  return JSON.stringify({ endpoint: new URL(config.endpoint).toString().replace(/\/$/, ''), region: config.region, bucket: config.bucket, forcePathStyle: config.forcePathStyle });
}

function s3Config(): S3Config {
  if (!env.S3_ENDPOINT || !env.S3_REGION || !env.S3_BUCKET || !env.S3_ACCESS_KEY_ID || !env.S3_SECRET_ACCESS_KEY) error(503, 'uploadUnavailable');
  return { endpoint: env.S3_ENDPOINT, region: env.S3_REGION, bucket: env.S3_BUCKET, accessKeyId: env.S3_ACCESS_KEY_ID, secretAccessKey: env.S3_SECRET_ACCESS_KEY, forcePathStyle: env.S3_FORCE_PATH_STYLE !== 'false' };
}

export function uploadStorage(task?: { transport: string; bucketIdentity: string }): UploadStorage {
  const mode = task?.transport ?? uploadTransport();
  if (!mode || (task && task.bucketIdentity !== storageIdentity(mode === 's3' ? 's3' : 'local'))) error(503, 'uploadUnavailable');
  if (mode === 's3') return createS3Storage(s3Config());
  if (mode !== 'local' || !dev || !env.STORAGE) error(503, 'uploadUnavailable');
  const bucket = env.STORAGE;
  return {
    async get(key, snapshot) {
      const object = await bucket.get(key, snapshot ? { onlyIf: { etagMatches: snapshot.etag } } : undefined);
      if (!object || !('body' in object) || (snapshot?.version && object.version !== snapshot.version)) throw new Error('processingSourceChanged');
      return new Response(object.body, { headers: { 'Content-Length': String(object.size) } });
    },
    async put(key, bytes) { await bucket.put(key, bytes); },
    head: (key) => bucket.head(key), delete: (key) => bucket.delete(key),
    createMultipartUpload: (key, taskId) => bucket.createMultipartUpload(key, {
      httpMetadata: { contentType: 'application/octet-stream' }, customMetadata: { uploadTask: taskId }
    }),
    resumeMultipartUpload: (key, uploadId) => bucket.resumeMultipartUpload(key, uploadId)
  };
}

export async function partGrant(task: { id: string; objectKey: string; multipartId: string | null; expiresAt: number; transport: string; bucketIdentity: string }, partNumber: number, size: number): Promise<PartGrant> {
  uploadStorage(task); // Reject a configuration change before authorizing a different bucket.
  if (task.transport === 'local') return { url: `/api/uploads/${task.id}?partNumber=${partNumber}`, headers: { 'Content-Type': 'application/octet-stream' } };
  return signS3Part(s3Config(), task.objectKey, task.multipartId!, partNumber, size,
    Math.max(1, Math.min(600, Math.floor((task.expiresAt - Date.now()) / 1000))));
}

export async function readBoundedBody(request: Request, expectedSize: number): Promise<Uint8Array> {
  if (!request.body) error(400, 'uploadInvalid');
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let received = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      received += value.byteLength;
      if (received > expectedSize) { await reader.cancel(); error(413, 'uploadInvalid'); }
      chunks.push(value);
    }
  } finally { reader.releaseLock(); }
  if (received !== expectedSize) error(400, 'uploadInvalid');
  const result = new Uint8Array(received);
  let offset = 0;
  for (const chunk of chunks) { result.set(chunk, offset); offset += chunk.byteLength; }
  return result;
}
