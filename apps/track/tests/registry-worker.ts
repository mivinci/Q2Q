// Test-only fault controls around the real named Registry entrypoint and SQL.
import { DurableObject } from 'cloudflare:workers';
import { PunchDiscovery as ActualDiscovery } from '../../registry/src/worker.ts';
import app from '../../registry/src/index.ts';
import type { RegistryEnv } from '../../registry/src/auth.ts';
let calls = 0, mode = 'ok';
type TestEnv = RegistryEnv['Bindings'] & {GATE: DurableObjectNamespace<Gate>};
export class Gate extends DurableObject {
  readonly #pending: (() => void)[] = [];
  wait() {return new Promise<void>(resolve => this.#pending.push(resolve));}
  release() {for (const resolve of this.#pending.splice(0)) resolve();}
}

export class PunchDiscovery extends ActualDiscovery {
  override async getPunches(input: unknown) {
    calls++;
    if (mode === 'fail' || (mode === 'fail-second' && !!(input as {after?:string}).after)) throw new Error('test-private-upstream-failure');
    if (mode === 'timeout') await (this.env as TestEnv).GATE.getByName('rpc-gate').wait();
    const page = await super.getPunches(input);
    if (mode === 'wrong-env') return {...page, environment: 'other-environment'};
    if (mode === 'bad-cursor') return {...page, next_cursor: 'not-the-last-node'};
    return page;
  }
}
export default {
  async fetch(request: Request, env: TestEnv) {
    const url = new URL(request.url);
    if (url.pathname === '/test-mode') {
      mode = url.searchParams.get('mode') ?? 'ok';
      if (mode !== 'timeout') await env.GATE.getByName('rpc-gate').release();
      return Response.json({calls});
    }
    if (url.pathname === '/test-stats') return Response.json({calls});
    return app.fetch(request, env);
  }
};
