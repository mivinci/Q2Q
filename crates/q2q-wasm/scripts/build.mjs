import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = resolve(root, 'crates/q2q-wasm/pkg');
for (const [command, args] of [
  ['cargo', ['build', '--locked', '-p', 'q2q-wasm', '--target', 'wasm32-unknown-unknown', '--release']],
  ['wasm-bindgen', [resolve(root, 'target/wasm32-unknown-unknown/release/q2q_wasm.wasm'), '--target', 'web', '--out-dir', output]]
]) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
mkdirSync(output, { recursive: true });
writeFileSync(resolve(output, 'package.json'), '{"type":"module"}\n');
