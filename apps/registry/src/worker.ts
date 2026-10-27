import { WorkerEntrypoint } from 'cloudflare:workers';
import { environmentSchema } from '@q2q/punch-discovery';
import { discoverPage } from './nodes.ts';
import type { RegistryEnv } from './auth.ts';

/** Only the named Service Binding grants access; no HTTP or mutation methods. */
export class PunchDiscovery extends WorkerEntrypoint<RegistryEnv['Bindings']> {
  async getPunches(input: unknown) {
    const environment = environmentSchema.safeParse(this.env.DISCOVERY_ENVIRONMENT);
    if (!environment.success) throw new Error('discovery_unavailable');
    try {
      const result = await discoverPage(this.env.DB, input);
      const server_time = Math.floor(Date.now() / 1000);
      const expires_at = Math.min(server_time + 60, ...result.nodes.map(node => node.expires_at));
      if (expires_at <= server_time) throw new Error('discovery_unavailable');
      return {version: 1 as const, environment: environment.data, ...result, server_time, expires_at};
    } catch {throw new Error('discovery_unavailable');}
  }
}

export { default } from './index.ts';
