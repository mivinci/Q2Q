const BLOCK = 262144;
export function httpFixture() {
  const proofs = new Map();
  const records = new Map();
  const activity = new Map();
  return async (request, response) => {
    const url = new URL(request.url, 'http://localhost');
    if (!url.pathname.startsWith('/fixture/')) return false;
    const key = url.searchParams.get('key');
    const kind = url.pathname.slice('/fixture/'.length);
    if (kind !== 'hidden') {
      response.setHeader('Access-Control-Allow-Origin', '*');
      response.setHeader('Access-Control-Expose-Headers', 'Content-Range');
    } else response.setHeader('Access-Control-Allow-Origin', '*');
    if (kind === 'register') {
      const chunks = []; for await (const chunk of request) chunks.push(chunk);
      proofs.set(key, Buffer.concat(chunks)); records.set(key, []);
      response.end('ok'); return true;
    }
    if (kind === 'stats') { response.end(JSON.stringify({ records: records.get(key) ?? [], peak: activity.get(key)?.peak ?? 0 })); return true; }
    const logs = records.get(key) ?? []; records.set(key, logs);
    logs.push({ kind, range: request.headers.range, cookie: request.headers.cookie, referer: request.headers.referer });
    if (kind === 'proof' || kind === 'badproof') {
      const value = Buffer.from(proofs.get(key) ?? []);
      if (kind === 'badproof' && value.length) value[0] ^= 255;
      response.end(value); return true;
    }
    if (kind === 'auth') { response.writeHead(401).end(); return true; }
    if (kind === 'unavailable' || (kind === 'retry' && logs.filter(item => item.kind === kind).length === 1)) {
      response.writeHead(503).end(); return true;
    }
    if (kind === 'redirect') { response.writeHead(302, { Location: `/fixture/good?${url.searchParams}` }).end(); return true; }
    if (kind === 'changed') { response.writeHead(416).end(); return true; }
    const counter = activity.get(key) ?? { active: 0, peak: 0 }; activity.set(key, counter);
    counter.active++; counter.peak = Math.max(counter.peak, counter.active);
    response.once('close', () => { counter.active--; });
    const size = Number(url.searchParams.get('size') ?? 2 * BLOCK + 17);
    const match = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? '');
    const start = match ? Number(match[1]) : 0;
    const end = match ? Number(match[2]) : size - 1;
    const full = kind === 'full';
    let bytes = Buffer.alloc(full ? size : end - start + 1);
    for (let i = 0; i < bytes.length; i++) { const offset = i + (full ? 0 : start); bytes[i] = (offset * 31 + Math.floor(offset / BLOCK)) % 251; }
    if (kind === 'corrupt') bytes[0] ^= 255;
    if (!full) response.writeHead(206, { 'Content-Range': `bytes ${kind === 'badrange' ? start + 1 : start}-${end}/${size}` });
    if (kind === 'short') bytes = bytes.subarray(1);
    if (kind === 'overflow') bytes = Buffer.concat([bytes, Buffer.from([0])]);
    const send = () => { if (!response.destroyed) response.end(bytes); };
    if (kind === 'slow') setTimeout(send, 250);
    else if (kind === 'stall') { response.flushHeaders(); setTimeout(send, 2000); }
    else send();
    return true;
  };
}
