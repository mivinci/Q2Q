import type { PartGrant, UploadedPart } from './upload.js';

export interface UploadProgress {
  id: string; name: string; size: number; sent: number;
  phase: 'initializing' | 'uploading' | 'completing';
}

export async function uploadRequest<T>(path: string, input?: unknown, method = 'POST', signal?: AbortSignal): Promise<T> {
  const response = await fetch(path, {
    method, credentials: 'same-origin', headers: { 'Content-Type': 'application/json' },
    body: input === undefined ? undefined : JSON.stringify(input), signal
  });
  const result = await response.json().catch(() => ({}));
  if (!response.ok) {
    const message = result && typeof result === 'object' && 'message' in result ? result.message : null;
    throw new Error(typeof message === 'string' ? message : 'uploadFailed');
  }
  return result as T;
}

function sendPart(grant: PartGrant, blob: Blob, signal: AbortSignal, progress: (sent: number) => void): Promise<string> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    const aborted = () => xhr.abort();
    const finish = () => signal.removeEventListener('abort', aborted);
    xhr.open('PUT', grant.url);
    xhr.timeout = 120000;
    for (const [key, value] of Object.entries(grant.headers)) xhr.setRequestHeader(key, value);
    xhr.upload.onprogress = (event) => progress(Math.min(event.loaded, blob.size));
    xhr.onload = () => {
      finish();
      const etag = xhr.getResponseHeader('ETag');
      if (xhr.status >= 200 && xhr.status < 300 && etag) resolve(etag.replace(/^"|"$/g, ''));
      else reject(new Error('uploadPartFailed'));
    };
    xhr.onerror = xhr.ontimeout = () => { finish(); reject(new Error('uploadPartFailed')); };
    xhr.onabort = () => { finish(); reject(new DOMException('Aborted', 'AbortError')); };
    signal.addEventListener('abort', aborted, { once: true });
    if (signal.aborted) { finish(); reject(new DOMException('Aborted', 'AbortError')); return; }
    xhr.send(blob);
  });
}

export async function uploadFile(file: File, projectId: string, signal: AbortSignal, progress: (value: UploadProgress) => void, existingFileId?: string) {
  const id = crypto.randomUUID();
  const fileId = existingFileId ?? crypto.randomUUID();
  const controller = new AbortController();
  const abort = () => controller.abort();
  signal.addEventListener('abort', abort, { once: true });
  if (signal.aborted) controller.abort();
  const report = (sent: number, phase: UploadProgress['phase']) => progress({ id, name: file.name, size: file.size, sent, phase });
  report(0, 'initializing');
  let created = false;
  let completing = false;
  try {
    // Do not abandon initialization mid-flight: obtain the task first, then honor cancellation.
    if (!existingFileId) await uploadRequest('/api/files', { id: fileId, projectId, name: file.name, size: file.size });
    const task = await uploadRequest<{ partSize: number; status: string }>(`/api/files/${fileId}/uploads`, { id });
    created = true;
    if (controller.signal.aborted) throw new DOMException('Aborted', 'AbortError');
    if (task.status !== 'uploading') throw new Error('uploadConflict');
    const count = Math.ceil(file.size / task.partSize);
    const sent = Array<number>(count).fill(0);
    const parts: UploadedPart[] = Array(count);
    let next = 0;
    const update = () => report(sent.reduce((sum, value) => sum + value, 0), 'uploading');
    const worker = async () => {
      while (next < count) {
        const index = next++;
        const blob = file.slice(index * task.partSize, Math.min((index + 1) * task.partSize, file.size));
        let lastError: unknown;
        for (let attempt = 0; attempt < 3; attempt++) {
          if (controller.signal.aborted) throw new DOMException('Aborted', 'AbortError');
          try {
            const grant = await uploadRequest<PartGrant>(`/api/uploads/${id}/parts`, { partNumber: index + 1 }, 'POST', controller.signal);
            const etag = await sendPart(grant, blob, controller.signal, (value) => { sent[index] = value; update(); });
            parts[index] = { partNumber: index + 1, etag };
            sent[index] = blob.size;
            update();
            lastError = null;
            break;
          } catch (cause) {
            if (controller.signal.aborted) throw cause;
            lastError = cause;
            sent[index] = 0;
            update();
          }
        }
        if (lastError) throw lastError;
      }
    };
    const workers = Array.from({ length: Math.min(3, count) }, worker);
    try { await Promise.all(workers); }
    catch (cause) { controller.abort(); await Promise.allSettled(workers); throw cause; }
    if (controller.signal.aborted) throw new DOMException('Aborted', 'AbortError');
    completing = true;
    report(file.size, 'completing');
    // Once completion starts, let the durable server transition finish even if the view leaves.
    await uploadRequest(`/api/uploads/${id}/complete`, { parts });
    return id;
  } catch (cause) {
    if (signal.aborted && created && !completing) {
      await uploadRequest(`/api/uploads/${id}`, undefined, 'DELETE');
      throw new DOMException('Aborted', 'AbortError');
    }
    throw cause;
  } finally {
    signal.removeEventListener('abort', abort);
  }
}
