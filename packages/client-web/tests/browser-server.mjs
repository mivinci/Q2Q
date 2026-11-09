import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve, sep } from 'node:path';
import ts from 'typescript';
import { build } from 'esbuild';

export async function startServer(options = {}) {
const root = resolve(import.meta.dirname, '..');
const discovery = await build({entryPoints:[resolve(root,'../punch-discovery/index.ts')],bundle:true,write:false,format:'esm',platform:'browser',target:'es2022'});
const server = createServer(async (request, response) => {
  try {
    if (options.handleRequest && await options.handleRequest(request, response)) return;
    const pathname = new URL(request.url, 'http://localhost').pathname;
    if (pathname === '/') { response.end('<!doctype html><title>SDK storage test</title>'); return; }
    if (pathname === '/manual-result' && request.method === 'POST' && options.onManualResult) {
      const expectedOrigin = `http://127.0.0.1:${server.address().port}`;
      if (request.headers.origin !== expectedOrigin || request.headers['content-type'] !== 'application/json') {
        response.writeHead(403).end(); return;
      }
      let body = '';
      for await (const bytes of request) {
        body += bytes.toString();
        if (body.length > 4096) { response.writeHead(413).end(); return; }
      }
      options.onManualResult(JSON.parse(body));
      response.end('ok'); return;
    }
    if (pathname === '/manual-save') {
      response.setHeader('Content-Type', 'text/html; charset=utf-8');
      response.end(`<!doctype html><meta charset="utf-8"><title>Q2Q storage save test</title>
        <h1>Q2Q：系统文件保存测试</h1><p>仅在临时测试目录创建 saved.bin。</p>
        <button id="save">选择位置并写入测试文件</button><pre id="status">等待保存</pre>
        <script type="module">import {install} from '/tests/manual-save.browser.ts'; install();</script>`);
      return;
    }
    if (pathname === '/punch-discovery.js') {response.setHeader('Content-Type','text/javascript');response.end(discovery.outputFiles[0].text);return;}
    if (pathname.startsWith('/wasm/')) {
      const wasmRoot = resolve(root, '../../crates/q2q-wasm/pkg');
      const path = resolve(wasmRoot, pathname.slice('/wasm/'.length));
      if (!path.startsWith(wasmRoot + sep) || !/\.(wasm|js)$/.test(path)) { response.writeHead(404).end(); return; }
      response.setHeader('Content-Type', path.endsWith('.wasm') ? 'application/wasm' : 'text/javascript');
      response.end(await readFile(path));
      return;
    }
    const path = resolve(root, '.' + pathname);
    if (!path.startsWith(root + sep) || !path.endsWith('.ts')) { response.writeHead(404).end(); return; }
    const source = await readFile(path, 'utf8');
    response.setHeader('Content-Type', 'text/javascript');
    response.end(ts.transpileModule(source, { compilerOptions: {
      target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext
    }}).outputText.replaceAll("'@q2q/punch-discovery'", "'/punch-discovery.js'").replaceAll('"@q2q/punch-discovery"', '"/punch-discovery.js"').replaceAll("'@q2q/wasm'", "'/wasm/q2q_wasm.js'").replaceAll('"@q2q/wasm"', '"/wasm/q2q_wasm.js"'));
  } catch { response.writeHead(500).end(); }
});
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  return { server, origin: `http://127.0.0.1:${address.port}` };
}
