import { Hono } from 'hono';
import { AuthError, ForbiddenError, RegistryConfigError, authentication, permissionRequired } from './auth.ts';
import { RegistryError, report, discover, display } from './nodes.ts';
import type { RegistryEnv } from './auth.ts';

export const app = new Hono<RegistryEnv>();

app.use('*', async (c, next) => {
  c.header('Cache-Control', 'no-store');
  c.header('X-Content-Type-Options', 'nosniff');
  await next();
});

app.onError((error, c) => {
  if (error instanceof AuthError || error instanceof ForbiddenError || error instanceof RegistryConfigError || error instanceof RegistryError) {
    return c.json({ code: error.code, detail: error.message }, error.status);
  }
  // Avoid logging raw requests, credentials, database errors or token claims.
  console.error('Registry request failed unexpectedly');
  return c.json({ code: 'internal_error', detail: 'Registry could not process this request' }, 500);
});

// Liveness only: this does not claim that D1 or any registered service is healthy.
app.get('/health', c => c.json({ service: 'q2q-registry', status: 'ok' }));
app.use('/v1/*', authentication());
app.put('/v1/nodes/self', permissionRequired('nodes:write:self'), report);
app.get('/v1/discovery/punches', permissionRequired('discovery:punch:read'), discover);
app.get('/v1/nodes', permissionRequired('nodes:read:display'), display);
app.notFound(c => c.json({ code: 'not_found', detail: 'Unknown Registry endpoint' }, 404));

export default app;
