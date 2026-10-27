// Test-only counters around actual SQL cursors and production DO methods.
import app from '../src/index.ts';
import { PeerState } from '../src/peer-state.ts';
import { FileIndex } from '../src/file-index.ts';
import { DiscoveryState } from '../src/directory.ts';
import { RateLimiter } from '../src/rate-limit.ts';
import type { Bindings, QueryContext } from '../src/state.ts';
import type { IndexEvent } from '../src/state.ts';

import { instrument } from './benchmark-meter.ts';
import type { Meter } from './benchmark-meter.ts';

export class MeasuredPeer extends PeerState {
  readonly #meter: Meter;
  constructor(ctx: DurableObjectState, env: Bindings) {super(ctx, env); this.#meter = instrument(ctx.storage);}
  override async announce(input: Parameters<PeerState['announce']>[0]) {this.#meter.counts.rpc++; return super.announce(input);}
  override async heartbeat(input: Parameters<PeerState['heartbeat']>[0]) {this.#meter.counts.rpc++; return super.heartbeat(input);}
  override async authorizeQuery(input: Parameters<PeerState['authorizeQuery']>[0], hash: string, deadline: number) {this.#meter.counts.rpc++; return super.authorizeQuery(input, hash, deadline);}
  override async candidate(hash: string, revision: string, context?: QueryContext) {this.#meter.counts.rpc++; return super.candidate(hash, revision, context);}
  override async alarm() {this.#meter.counts.alarms++; await super.alarm();}
  measure(reset: boolean) {return this.#meter.snapshot(reset);}
}
export class MeasuredFile extends FileIndex {
  readonly #meter: Meter;
  constructor(ctx: DurableObjectState, env: Bindings) {super(ctx, env); this.#meter = instrument(ctx.storage);}
  override async apply(event: IndexEvent) {this.#meter.counts.rpc++; return super.apply(event);}
  override async page(hash: string, after: string | null, count: number, context?: QueryContext) {this.#meter.counts.rpc++; return super.page(hash, after, count, context);}
  override async alarm() {this.#meter.counts.alarms++; await super.alarm();}
  measure(reset: boolean) {return this.#meter.snapshot(reset);}
}
export class MeasuredDiscovery extends DiscoveryState {
  readonly #meter: Meter;
  constructor(ctx: DurableObjectState, env: Bindings) {super(ctx, env); this.#meter = instrument(ctx.storage);}
  override async snapshot() {this.#meter.counts.rpc++; return super.snapshot();}
  override async alarm() {this.#meter.counts.alarms++; await super.alarm();}
  measure(reset: boolean) {return this.#meter.snapshot(reset);}
}
export class MeasuredLimiter extends RateLimiter {
  readonly #meter: Meter;
  constructor(ctx: DurableObjectState, env: Bindings) {super(ctx, env); this.#meter = instrument(ctx.storage);}
  override async take(kind: 'announce' | 'session') {this.#meter.counts.rpc++; return super.take(kind);}
  override async alarm() {this.#meter.counts.alarms++; await super.alarm();}
  measure(reset: boolean) {return this.#meter.snapshot(reset);}
}
interface MeasuredBindings extends Bindings {
  PEERS: DurableObjectNamespace<MeasuredPeer>; FILES: DurableObjectNamespace<MeasuredFile>;
  DISCOVERY: DurableObjectNamespace<MeasuredDiscovery>; RATE_LIMIT: DurableObjectNamespace<MeasuredLimiter>;
};
export default {
  async fetch(request: Request, env: MeasuredBindings) {
    if (new URL(request.url).pathname === '/measure') {
      const input = await request.json() as {kind:'PEERS'|'FILES'|'DISCOVERY'|'RATE_LIMIT'; name:string; reset:boolean};
      return Response.json(await env[input.kind].getByName(input.name).measure(input.reset));
    }
    return app.fetch(request, env);
  }
};
