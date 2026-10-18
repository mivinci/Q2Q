import { error, type RequestEvent } from '@sveltejs/kit';

export function uploadUser(event: RequestEvent): string {
  if (!event.locals.user) error(401, 'uploadSignInRequired');
  if (event.request.method !== 'GET' && event.request.headers.get('origin') !== event.url.origin) error(403, 'uploadForbidden');
  return event.locals.user.id;
}

export async function uploadJson(request: Request): Promise<Record<string, unknown>> {
  if (!request.headers.get('content-type')?.startsWith('application/json') || !request.body) error(400, 'uploadInvalid');
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > 160 * 1024) { await reader.cancel(); error(413, 'uploadInvalid'); }
      chunks.push(value);
    }
  } finally { reader.releaseLock(); }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  let input: unknown;
  try { input = JSON.parse(new TextDecoder().decode(bytes)); } catch { error(400, 'uploadInvalid'); }
  if (!input || typeof input !== 'object' || Array.isArray(input)) error(400, 'uploadInvalid');
  return input as Record<string, unknown>;
}
