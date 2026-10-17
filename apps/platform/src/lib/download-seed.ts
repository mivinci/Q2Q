import init, { seed_with_source } from '@q2q/wasm';
import wasmUrl from '@q2q/wasm/module.wasm?url';
let initialized: ReturnType<typeof init> | undefined;
export async function downloadSeed(bytes: Uint8Array, source: string, name: string) {
  await (initialized ??= init({ module_or_path: wasmUrl }).catch((cause) => { initialized = undefined; throw cause; }));
  const seed = seed_with_source(bytes, source);
  const url = URL.createObjectURL(new Blob([new Uint8Array(seed).buffer], { type: 'application/octet-stream' }));
  const anchor = document.createElement('a');
  anchor.href = url; anchor.download = `${name}.q2q`; anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
