import { AwsClient, AwsV4Signer } from 'aws4fetch';
import { XMLParser } from 'fast-xml-parser';
import type { PartGrant, UploadedPart } from '../upload.js';

export interface S3Config {
  endpoint: string; region: string; bucket: string; accessKeyId: string; secretAccessKey: string;
  forcePathStyle?: boolean;
}
export interface StoredObject {
  size: number; etag: string; version: string | null; customMetadata?: Record<string, string>;
}
export interface UploadStorage {
  get(key: string, snapshot?: { etag: string; version: string | null }): Promise<Response>;
  put(key: string, bytes: Uint8Array): Promise<void>;
  head(key: string): Promise<StoredObject | null>;
  delete(key: string): Promise<void>;
  createMultipartUpload(key: string, taskId: string): Promise<{ uploadId: string; abort(): Promise<void> }>;
  resumeMultipartUpload(key: string, uploadId: string): {
    abort(): Promise<void>;
    complete(parts: UploadedPart[]): Promise<StoredObject>;
  };
}

const parser = new XMLParser({ ignoreAttributes: true, processEntities: false, parseTagValue: false });
const escapeXml = (value: string) => value.replace(/[<>&"']/g, (char) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;', "'": '&apos;' })[char]!);

export function s3ObjectUrl(config: S3Config, key: string): URL {
  const endpoint = new URL(config.endpoint);
  const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(endpoint.hostname);
  if ((endpoint.protocol !== 'https:' && !(endpoint.protocol === 'http:' && loopback)) ||
      endpoint.username || endpoint.password || endpoint.search || endpoint.hash ||
      !/^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/.test(config.bucket) ||
      key.split('/').some((segment) => segment === '.' || segment === '..')) throw new Error('Invalid S3 configuration');
  const prefix = endpoint.pathname.replace(/\/$/, '');
  if (config.forcePathStyle !== false) endpoint.pathname = `${prefix}/${encodeURIComponent(config.bucket)}/${key.split('/').map(encodeURIComponent).join('/')}`;
  else {
    endpoint.hostname = `${config.bucket}.${endpoint.hostname}`;
    endpoint.pathname = `${prefix}/${key.split('/').map(encodeURIComponent).join('/')}`;
  }
  return endpoint;
}

export async function signS3Part(config: S3Config, key: string, uploadId: string, partNumber: number, size: number, expiresIn: number): Promise<PartGrant> {
  const url = s3ObjectUrl(config, key);
  url.searchParams.set('partNumber', String(partNumber));
  url.searchParams.set('uploadId', uploadId);
  url.searchParams.set('X-Amz-Expires', String(expiresIn));
  const signed = await new AwsV4Signer({
    method: 'PUT', url: url.toString(), region: config.region, service: 's3',
    accessKeyId: config.accessKeyId, secretAccessKey: config.secretAccessKey,
    headers: { 'Content-Length': String(size), 'Content-Type': 'application/octet-stream' },
    signQuery: true, allHeaders: true
  }).sign();
  return { url: signed.url.toString(), headers: { 'Content-Type': 'application/octet-stream' } };
}

export function createS3Storage(config: S3Config): UploadStorage {
  const client = new AwsClient({ accessKeyId: config.accessKeyId, secretAccessKey: config.secretAccessKey, region: config.region, service: 's3', retries: 0 });
  async function request(key: string, method: string, query: Record<string, string> = {}, headers?: HeadersInit, body?: string | Uint8Array) {
    const url = s3ObjectUrl(config, key);
    for (const [name, value] of Object.entries(query)) url.searchParams.set(name, value);
    // Workers supports manual redirects. Every caller rejects 3xx; credentials never follow them.
    return client.fetch(url.toString(), { method, headers, body: body instanceof Uint8Array ? new Uint8Array(body).buffer : body, redirect: 'manual' });
  }
  async function head(key: string): Promise<StoredObject | null> {
    const response = await request(key, 'HEAD');
    if (response.status === 404) return null;
    if (!response.ok) throw new Error(`S3 HEAD failed (${response.status})`);
    const length = response.headers.get('content-length');
    const size = length === null ? NaN : Number(length);
    const etag = response.headers.get('etag')?.replace(/^"|"$/g, '');
    if (!Number.isSafeInteger(size) || size < 0 || !etag) throw new Error('Invalid S3 object metadata');
    return { size, etag, version: response.headers.get('x-amz-version-id'), customMetadata: { uploadTask: response.headers.get('x-amz-meta-uploadtask') ?? '' } };
  }
  function resumeMultipartUpload(key: string, uploadId: string) {
    return {
      async abort() {
        const response = await request(key, 'DELETE', { uploadId });
        if (!response.ok && response.status !== 404) throw new Error(`S3 abort failed (${response.status})`);
        await response.body?.cancel();
      },
      async complete(parts: UploadedPart[]): Promise<StoredObject> {
        const body = `<CompleteMultipartUpload>${parts.map((part) => `<Part><PartNumber>${part.partNumber}</PartNumber><ETag>${escapeXml('"' + part.etag + '"')}</ETag></Part>`).join('')}</CompleteMultipartUpload>`;
        const response = await request(key, 'POST', { uploadId }, { 'Content-Type': 'application/xml' }, body);
        const xml = parser.parse(await response.text());
        // S3 may report an embedded error even with HTTP 200.
        if (!response.ok || !xml.CompleteMultipartUploadResult || xml.Error) throw new Error(`S3 completion failed (${response.status})`);
        const object = await head(key);
        if (!object) throw new Error('Completed S3 object not found');
        return object;
      }
    };
  }
  return {
    head, resumeMultipartUpload,
    async get(key, snapshot) {
      const response = await request(key, 'GET', snapshot?.version ? { versionId: snapshot.version } : {}, snapshot ? { 'If-Match': `"${snapshot.etag}"` } : undefined);
      if (!response.ok) { await response.body?.cancel(); throw new Error('processingSourceChanged'); }
      if (snapshot && (response.headers.get('etag')?.replace(/^"|"$/g, '') !== snapshot.etag ||
          (snapshot.version && response.headers.get('x-amz-version-id') !== snapshot.version))) {
        await response.body?.cancel(); throw new Error('processingSourceChanged');
      }
      return response;
    },
    async put(key, bytes) {
      const response = await request(key, 'PUT', {}, { 'Content-Type': 'application/octet-stream' }, bytes);
      if (!response.ok) throw new Error('processingStorageFailed');
      await response.body?.cancel();
    },
    async delete(key) {
      const response = await request(key, 'DELETE');
      if (!response.ok && response.status !== 404) throw new Error(`S3 deletion failed (${response.status})`);
      await response.body?.cancel();
    },
    async createMultipartUpload(key, taskId) {
      const response = await request(key, 'POST', { uploads: '' }, { 'Content-Type': 'application/octet-stream', 'x-amz-meta-uploadTask': taskId });
      const xml = parser.parse(await response.text());
      const uploadId: unknown = xml.InitiateMultipartUploadResult?.UploadId;
      if (!response.ok || typeof uploadId !== 'string' || !uploadId) throw new Error(`S3 initialization failed (${response.status})`);
      return { uploadId, abort: () => resumeMultipartUpload(key, uploadId).abort() };
    }
  };
}
