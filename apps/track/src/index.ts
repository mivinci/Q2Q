import { Hono } from 'hono';
import { api } from './http.ts';
import type { Bindings } from './state.ts';

export { RateLimiter } from './rate-limit.ts';
export { PeerState } from './peer-state.ts';
export { FileIndex } from './file-index.ts';
export { DiscoveryState } from './directory.ts';

const app = new Hono<{ Bindings: Bindings }>();
app.get('/health', context => {
  context.header('Cache-Control', 'no-store');
  context.header('Access-Control-Allow-Origin', '*');
  return context.json({ service: 'q2q-track', version: '0.0.0' });
});
app.route('/', api);
app.notFound(context => {
  const known = /^(?:\/health|\/peers\/[^/]+\/(?:files|heartbeat)|\/files\/[^/]+\/peers)$/.test(context.req.path);
  return context.json({code: known ? 'method_not_allowed' : 'not_found'}, known ? 405 : 404);
});
app.onError((error, context) => {
  context.header('Cache-Control', 'no-store');
  switch (error.message) {
    case 'rate_limited': return context.json({ code: error.message }, 429);
    case 'unprocessable_entity': return context.json({ code: error.message }, 422);
    case 'bad_request': case 'malformed_ticket': return context.json({ code: error.message }, 400);
    case 'bad_signature': case 'key_not_issuer': case 'expired': case 'lifetime_too_long':
    case 'wrong_audience': case 'bad_mac': case 'stale_timestamp': case 'unknown_session':
      return context.json({ code: error.message }, 401);
    case 'storage_unavailable': case 'discovery_unavailable': case 'no_available_punch': return context.json({ code: error.message }, 503);
    default: return context.json({ code: 'internal_error' }, 500);
  }
});
export default app;
