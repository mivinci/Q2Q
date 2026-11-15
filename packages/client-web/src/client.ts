import {partialSharing} from './partial-sharing.ts';
import type {PartialSharing} from './partial-sharing.ts';
import { createPublication } from './publication.ts';
import { Q2QError } from './error.ts';
import { peerSource } from './peer-download.ts';
import { peerNetwork } from './network.ts';
import { abortable, PermitPool } from './http.ts';
import { initializeWasm } from './block-verifier.ts';
import { parseSeed, seedData } from './seed.ts';
import { parseMagnetLink, cachedMagnetSeed, resolveMagnet } from './magnet.ts';
import type { MagnetLink } from './client-types.ts';
import { controlIdentity } from './control-identity.ts';
import { connectNetwork } from './network.ts';
import type { NetworkSession } from './network.ts';
import type { ControlIdentity } from './control-identity.ts';
import { memoryState } from './memory-state.ts';
import { createTransfer } from './transfer.ts';
import type { Client, ClientOptions, ClientLimits, DownloadOptions, Transfer, Publication } from './client-types.ts';

export function createClient(options: ClientOptions = {}): Client {
  const limits: ClientLimits = { httpRequests: 2, peerConnections:2, uploadConnections:2, uploadReads:1, maxTasks: 4, commitBytes: 16 * 1024 * 1024,
    requestTimeoutMs: 30000, attemptsPerSource: 2, retryDelayMs: 100, ...options.limits };
  for (const [key, value] of Object.entries(limits)) {
    if (!Number.isSafeInteger(value) || value < (key === 'retryDelayMs' ? 0 : 1)) throw new Q2QError('INVALID_ARGUMENT');
  }
  if (limits.uploadConnections > 16 || limits.uploadReads > 16 || limits.peerConnections > 16 || limits.httpRequests > 16 || limits.maxTasks > 64 || limits.attemptsPerSource > 8) throw new Q2QError('RESOURCE_LIMIT');
  const providedState = options.state;
  const sourceProvider = options.sourceProvider;
  const wasmUrl = options.wasmUrl;
  const trackUrl = options.trackUrl;
  let identity: ControlIdentity | undefined;
  let identityWork: Promise<ControlIdentity> | undefined;
  let networkWork: Promise<NetworkSession> | undefined;
  let network: NetworkSession | undefined;
  const state = providedState ?? memoryState();
  const pool = new PermitPool(limits.httpRequests);
  const peerPermits=new PermitPool(limits.peerConnections);
  const controller = new AbortController();
  const tasks = new Set<Transfer>();
  const publications=new Set<Publication>();const uploadReads=new PermitPool(limits.uploadReads);const uploadPermits=new PermitPool(limits.uploadConnections);
  const parsing = new Set<Promise<unknown>>();
  let closing: Promise<void> | undefined;
  const check = () => { if (closing || controller.signal.aborted) throw new Q2QError('CLIENT_CLOSED'); };
  function connect():Promise<NetworkSession> {
      check();
      if (!trackUrl) return Promise.reject(new Q2QError('INVALID_ARGUMENT'));
      if (networkWork) return networkWork;
      if (network?.snapshot().state === 'online') return Promise.resolve(network);
      const work = (async () => {
        await network?.close();
        identityWork ??= controlIdentity(wasmUrl,controller.signal).then(value => { identity = value; return value; }).catch(error => { identityWork = undefined; throw error; });
        const own = await identityWork;
        if (controller.signal.aborted) throw new Q2QError('CLIENT_CLOSED');
        network = await connectNetwork({ identity:own, trackUrl, signal:controller.signal, timeoutMs:Math.min(limits.requestTimeoutMs,10000) });
        return network;
      })();
      networkWork = work;
      void work.finally(() => { if (networkWork === work) networkWork = undefined; }).catch(() => {});
      return work;
    }
  function download(input: DownloadOptions, checkpointId?: string, sharing?:PartialSharing,stopSharing?:()=>Promise<void>): Transfer {
    check(); seedData(input.seed);
    if (!input.destination?.targetId || typeof input.destination.open !== 'function') throw new Q2QError('INVALID_ARGUMENT');
    if (tasks.size+publications.size >= limits.maxTasks) throw new Q2QError('RESOURCE_LIMIT');
    const task = createTransfer({ ...input, checkpointId, sharing,stopSharing,state, pool, limits,
      sourceProvider, wasmUrl,
      ...(trackUrl&&typeof RTCPeerConnection==='function'?{openPeers:async(committed:Uint8Array,signal:AbortSignal,received:(bytes:number)=>void)=>{
        const session=await abortable(connect(),signal);
        return peerSource({network:peerNetwork(session),content:input.seed.info,committed,signal,permits:peerPermits,received});
      }}:{}), ended: () => tasks.delete(task) });
    tasks.add(task); return task;
  }
  function sharedDownload(input:DownloadOptions,checkpointId?:string){
      check();seedData(input.seed);
      if(!trackUrl||typeof RTCPeerConnection!=='function')throw new Q2QError('UNSUPPORTED_CAPABILITY');
      if(input.seed.info.blockCount>65536n||publications.size||tasks.size+1>=limits.maxTasks)throw new Q2QError('RESOURCE_LIMIT');
      if(!input.destination?.targetId||typeof input.destination.open!=='function')throw new Q2QError('INVALID_ARGUMENT');
      const sharing=partialSharing(Number(input.seed.info.blockCount));
      const source={info:{name:input.seed.info.name??'shared-download',size:input.seed.info.size},async open():Promise<never>{throw new Q2QError('INVALID_STATE');}};
      const publication=createPublication({source,preparedSeed:input.seed,blocks:sharing.access,wasmUrl,connect,permits:uploadPermits,reads:uploadReads,maxConnections:limits.uploadConnections,ended:()=>publications.delete(publication)});
      publications.add(publication);
      const transfer=download(input,checkpointId,sharing,()=>publication.close());
      return Object.freeze({transfer,publication});
  }
  return Object.freeze({
    capabilities: Object.freeze({ http: true as const, p2p: Boolean(trackUrl&&typeof RTCPeerConnection==='function'), publish: Boolean(trackUrl&&typeof RTCPeerConnection==='function'),
      checkpointStore: providedState ? 'provided' as const : 'memory' as const }),
    connectNetwork:connect,
    publish(input){
      check();
      if(!trackUrl||typeof RTCPeerConnection!=='function')throw new Q2QError('UNSUPPORTED_CAPABILITY');
      if(!input?.source||typeof input.source.open!=='function'||typeof input.source.info?.size!=='bigint'||input.source.info.size<0n||input.source.info.size>65536n*262144n
        ||typeof input.source.info.name!=='string'||input.source.info.name.length>1024||input.name!==undefined&&(typeof input.name!=='string'||input.name.length>1024))throw new Q2QError('INVALID_ARGUMENT');
      if(publications.size||tasks.size+publications.size>=limits.maxTasks)throw new Q2QError('RESOURCE_LIMIT');
      const publication=createPublication({...input,wasmUrl,connect,permits:uploadPermits,reads:uploadReads,maxConnections:limits.uploadConnections,ended:()=>publications.delete(publication)});
      publications.add(publication);return publication;
    },
    parseSeed(input, settings = {}) {
      check();
      if (parsing.size >= limits.maxTasks) return Promise.reject(new Q2QError('RESOURCE_LIMIT'));
      const own = new AbortController();
      const abort = () => own.abort();
      controller.signal.addEventListener('abort', abort, { once: true });
      settings.signal?.addEventListener('abort', abort, { once: true });
      if (settings.signal?.aborted) own.abort();
      const work = parseSeed(input, own.signal, wasmUrl).finally(() => {
        controller.signal.removeEventListener('abort', abort); settings.signal?.removeEventListener('abort', abort); parsing.delete(work);
      });
      parsing.add(work); return work;
    },
    parseMagnet(link, settings = {}) {
      check();
      if (typeof link !== 'string' || link.length === 0 || link.length > 16 * 1024) return Promise.reject(new Q2QError('INVALID_MAGNET'));
      if (parsing.size >= limits.maxTasks) return Promise.reject(new Q2QError('RESOURCE_LIMIT'));
      const own = new AbortController();
      const abort = () => own.abort();
      controller.signal.addEventListener('abort', abort, { once: true });
      settings.signal?.addEventListener('abort', abort, { once: true });
      if (settings.signal?.aborted) own.abort();
      const work = parseMagnetLink(link, own.signal, wasmUrl).finally(() => {
        controller.signal.removeEventListener('abort', abort); settings.signal?.removeEventListener('abort', abort); parsing.delete(work);
      });
      parsing.add(work); return work;
    },
    async resolveMagnet(input, settings = {}) {
      check();
      const isLink = (value: unknown): value is MagnetLink =>
        !!value && typeof value === 'object' && typeof (value as MagnetLink).infoHash === 'string'
        && /^[a-f0-9]{64}$/.test((value as MagnetLink).infoHash)
        && ((value as MagnetLink).displayName === null || typeof (value as MagnetLink).displayName === 'string')
        && Array.isArray((value as MagnetLink).trackers);
      if (typeof input !== 'string' && !isLink(input)) throw new Q2QError('INVALID_ARGUMENT');
      if (typeof input === 'string' && (input.length === 0 || input.length > 16 * 1024)) throw new Q2QError('INVALID_MAGNET');
      if (parsing.size >= limits.maxTasks) throw new Q2QError('RESOURCE_LIMIT');
      const own = new AbortController();
      const abort = () => own.abort();
      controller.signal.addEventListener('abort', abort, { once: true });
      settings.signal?.addEventListener('abort', abort, { once: true });
      if (settings.signal?.aborted) own.abort();
      const work = (async () => {
        const link = typeof input === 'string' ? await parseMagnetLink(input, own.signal, wasmUrl) : input;
        // A verified cache hit needs no network; only discovery requires P2P.
        const cached = await cachedMagnetSeed(link, own.signal, wasmUrl);
        if (cached) return cached;
        if (!trackUrl || typeof RTCPeerConnection !== 'function') throw new Q2QError('UNSUPPORTED_CAPABILITY');
        return resolveMagnet({ link, connect, permits: peerPermits, signal: own.signal, wasmUrl });
      })().finally(() => {
        controller.signal.removeEventListener('abort', abort); settings.signal?.removeEventListener('abort', abort); parsing.delete(work);
      });
      parsing.add(work); return work;
    },
    async seedMagnet(seed) {
      check();
      // Read through the public handle method: UI frameworks may wrap the
      // handle in a proxy, which would miss the internal WeakMap entry.
      let bytes: Uint8Array | undefined;
      try {
        const candidate = seed as unknown as { toBytes?: () => Uint8Array };
        if (typeof candidate?.toBytes === 'function') bytes = candidate.toBytes();
      } catch { /* fall through to the identity lookup */ }
      if (!bytes) bytes = seedData(seed).bytes;
      const module = await initializeWasm(wasmUrl);
      try { return module.seed_magnet(bytes); }
      catch { throw new Q2QError('INVALID_SEED'); }
    },
    downloadShared:sharedDownload,
    resumeDownloadShared(input){
      if(typeof input.checkpointId!=='string'||!input.checkpointId)throw new Q2QError('INVALID_ARGUMENT');
      return sharedDownload(input,input.checkpointId);
    },
    download,
    resumeDownload(input) {
      if (typeof input.checkpointId !== 'string' || !input.checkpointId) throw new Q2QError('INVALID_ARGUMENT');
      return download(input, input.checkpointId);
    },
    async listCheckpoints(settings) { check(); return state.list(settings); },
    async discardCheckpoint(id) { check(); await state.discard(id); },
    close() {
      if (!closing) {
        closing = Promise.resolve().then(async () => {
          const publicationResults=await Promise.allSettled([...publications].map(publication=>publication.close()));
          controller.abort();
          const results = await Promise.allSettled([...tasks].map(task => task.cancel()));
          await Promise.allSettled([...parsing, ...(networkWork ? [networkWork] : []), ...(identityWork ? [identityWork] : [])]);
          try { await network?.close(); }
          finally {
            identity?.close();
            if (!providedState) await state.close();
          }
          const failure = [...results,...publicationResults].find(item => item.status === 'rejected');
          if (failure?.status === 'rejected') throw failure.reason;
        });
      }
      return closing;
    }
  } satisfies Client);
}
