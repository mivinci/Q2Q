import { fileDestination, StorageError } from '../src/index.ts';

declare global {
  interface Window {
    saveTestResult?: { ok: boolean; bytes?: number; kind?: string; hash?: string; error?: string };
  }
}
export function install() {
  const button = document.querySelector<HTMLButtonElement>('#save')!;
  const status = document.querySelector<HTMLElement>('#status')!;
  button.onclick = async () => {
    button.disabled = true;
    try {
      const picker = (window as unknown as {
        showSaveFilePicker(options: { suggestedName: string }): Promise<FileSystemFileHandle>;
      }).showSaveFilePicker;
      if (!picker) throw new StorageError('UNSUPPORTED_CAPABILITY');
      // Picker must be called directly in this user-activation event.
      const handle = await picker.call(window, { suggestedName: 'saved.bin' });
      const destination = await fileDestination(handle);
      const bytes = new Uint8Array(65553);
      for (let index = 0; index < bytes.length; index++) bytes[index] = (index * 13 + 7) % 251;
      const writer = await destination.open({ mode: 'create', size: BigInt(bytes.length) });
      try {
        await writer.write(32768n, bytes.subarray(32768));
        await writer.write(0n, bytes.subarray(0, 32768));
        const receipt = await writer.finish();
        const actual = new Uint8Array(await (await handle.getFile()).arrayBuffer());
        if (receipt.kind !== 'user-file' || !actual.every((value, i) => value === bytes[i]) || actual.length !== bytes.length) {
          throw new Error('saved content mismatch');
        }
        const hash = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', actual)),
          value => value.toString(16).padStart(2, '0')).join('');
        window.saveTestResult = { ok: true, bytes: actual.length, kind: receipt.kind, hash };
      } finally { await writer.abort(); }
    } catch (error) {
      window.saveTestResult = { ok: false, error: error instanceof StorageError ? error.code
        : error instanceof Error ? error.name : 'UNKNOWN' };
    }
    status.textContent = JSON.stringify(window.saveTestResult, null, 2);
    await fetch('/manual-result', { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(window.saveTestResult) });
    button.disabled = false;
  };
}
