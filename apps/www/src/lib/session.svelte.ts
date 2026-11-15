import {PUBLIC_Q2Q_TRACK_URL} from '$app/env/public';
import { SvelteMap } from 'svelte/reactivity';
import { browserState, createClient, fileSource, opfsCache, fileDestination, Q2QError } from '@q2q/client-web';
import type { Client, Seed, StateStore, OpfsCache, Transfer, DownloadSnapshot, Publication, PublicationSnapshot,ContentSource } from '@q2q/client-web';
import type { TextKey } from './messages.js';

export interface ShareRow { sourceCacheId?:string; id:string;name:string;size:string;snapshot:Readonly<PublicationSnapshot>;seed?:Seed;busy?:boolean;error?:TextKey;magnetCopied?:boolean }
const HISTORY = 'q2q-web-transfer-v1:';
interface Stored { id: string; cacheId: string; checkpointId: string | null; contentId: string; name: string; size: string; committed: string; complete: boolean; origin?: 'seed' | 'magnet' }
export interface Row extends Stored { networkState?:'connecting'|'online'|'offline'|'closed'; snapshot?: Readonly<DownloadSnapshot>; saved?: boolean; submitted?: boolean; saving?: boolean; exportBytes?: bigint; busy?: boolean; error?: TextKey; unavailable?: boolean }
/** Which re-supply action a restored row offers; the seed path also accepts a
 * matching .q2q file, so magnet rows keep that menu entry too. */
export function rowOrigin(row: Row): 'seed' | 'magnet' { return row.origin === 'magnet' ? 'magnet' : 'seed'; }
const active = new Set(['preparing', 'waiting', 'downloading', 'pausing', 'finalizing', 'cancelling']);
export function errorMessage(error: unknown): TextKey {
  if (!(error instanceof Q2QError)) return 'storage';
  switch (error.code) {
    case 'INVALID_SEED': return 'invalid';
    case 'RESOURCE_LIMIT': return 'tooMany';
    case 'CONTENT_MISMATCH': return 'corrupt';
    case 'AUTH_REQUIRED': return 'unauthorized';
    case 'PEER_CLOSED':case 'PEER_PROTOCOL_ERROR':case 'CONTROL_TIMEOUT':case 'PUNCH_UNAVAILABLE':case 'NETWORK_CLOSED':case 'NETWORK_UNAVAILABLE': case 'HTTP_STATUS': return 'network';
    case 'SOURCE_CHANGED': return 'sourceChanged';
    case 'HTTP_RANGE_UNSUPPORTED': case 'HTTP_PROTOCOL_ERROR': return 'range';
    case 'STORAGE_FULL': return 'space';
    case 'CACHE_NOT_FOUND': return 'dataCleared';
    case 'CHECKPOINT_MISMATCH': return 'mismatch';
    case 'DESTINATION_BUSY': case 'CHECKPOINT_BUSY': return 'busy';
    case 'NO_SOURCES': return 'noSources';
    case 'PROOF_UNAVAILABLE': return 'proofUnavailable';
    case 'INVALID_MAGNET': return 'magnetInvalid';
    case 'MAGNET_UNSUPPORTED': return 'magnetUnsupported';
    case 'METADATA_MISMATCH': return 'magnetBadMetadata';
    default: return 'storage';
  }
}
export function filename(name: string | null) { return (name ?? 'download').split(/[/\\]/).at(-1)!.replace(/[\x00-\x1f<>:"|?*]/g, '_').slice(0, 160) || 'download'; }

/** UI orchestration only: the SDK owns network, integrity and checkpoint state. */
export class WebSession {
  rows = $state<Row[]>([]);
  shares = $state<ShareRow[]>([]);
  sharePreview = $state.raw<File|null>(null);
  private publications=new Map<string,Publication>();
  /** Share seeds stay out of the reactive row: Svelte proxies would break the
   * SDK's WeakMap-backed handle identity. */
  private shareSeeds=new SvelteMap<string,Seed>();
  private sharingClient!:Client;
  networkEnabled=false;
  // Seed is an opaque SDK handle indexed by identity, not a mutable UI model.
  preview = $state.raw<Seed | null>(null);
  error = $state<TextKey | null>(null);
  historyError = $state(false);
  ready = $state(false);
  parsing = $state(false);
  /** Magnet dialog state: hash shown while resolving, typed failures after. */
  magnetResolving = $state(false);
  magnetHash = $state<string | null>(null);
  magnetError = $state<TextKey | null>(null);
  magnetLink = $state<string | null>(null);
  private client!: Client;
  private cache!: OpfsCache;
  private state!: StateStore;
  private handles = new Map<string, Transfer>();
  private seeds = new SvelteMap<string, Seed>();
  private exports = new Map<string, AbortController>();
  private closed = false;
  private selecting = 0;
  private resumeId: string | undefined;
  private resumeOrigin: 'seed' | 'magnet' = 'seed';
  private starting = false;
  private magnetAbort: AbortController | undefined;
  private copiedTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private closePromise?: Promise<void>;
  private urls = new Map<string, ReturnType<typeof setTimeout>>();
  private exportWork = new Set<Promise<void>>();
  async init() {
    try {
      this.state = await browserState({ namespace: 'q2q-web' });
      this.cache = await opfsCache();
      const trackUrl=PUBLIC_Q2Q_TRACK_URL;
      this.networkEnabled=!!trackUrl;
      this.client = createClient({ state: this.state, ...(trackUrl?{trackUrl}:{}) });
      this.sharingClient=createClient(trackUrl?{trackUrl}:{});
      // A record per transfer prevents unrelated tabs overwriting a shared list.
      try {
        const records: Stored[] = [];
        for (let i = 0; i < localStorage.length; i++) {
          const key = localStorage.key(i);
          if (!key?.startsWith(HISTORY)) continue;
          const raw = localStorage.getItem(key);
          if (!raw || raw.length > 4096) continue;
          try {
            const r = JSON.parse(raw);
            if (!r || typeof r !== 'object' || key !== HISTORY + r.id
              || !/^[a-f0-9-]{36}$/.test(r.id) || !/^[a-f0-9-]{36}$/.test(r.cacheId)
              || !(r.checkpointId === null || typeof r.checkpointId === 'string' && /^[a-f0-9-]{36}$/.test(r.checkpointId))
              || !/^sha256:[a-f0-9]{64}$/.test(r.contentId) || typeof r.name !== 'string' || r.name.length > 160
              || typeof r.size !== 'string' || !/^\d{1,16}$/.test(r.size) || BigInt(r.size) > BigInt(Number.MAX_SAFE_INTEGER)
              || typeof r.committed !== 'string' || !/^\d{1,16}$/.test(r.committed) || BigInt(r.committed) > BigInt(r.size)
              || typeof r.complete !== 'boolean' || !(r.origin === undefined || r.origin === 'seed' || r.origin === 'magnet')) continue;
            records.push({id:r.id,cacheId:r.cacheId,checkpointId:r.checkpointId,contentId:r.contentId,
              name:r.name,size:r.size,committed:r.committed,complete:r.complete,origin:r.origin});
          } catch { /* A corrupt history record cannot disable the downloader. */ }
          if (records.length >= 32) break;
        }
        this.rows = records;
      } catch { this.historyError = true; }
      for (const row of this.rows) {
        try { row.complete = await (await this.cache.open(row.cacheId)).status() === 'complete'; }
        catch (error) { row.error = errorMessage(error); row.unavailable = error instanceof Q2QError && error.code === 'CACHE_NOT_FOUND'; }
      }
      if (this.closed) { await this.client.close();await this.sharingClient.close(); await this.state.close(); return; }
      this.ready = true;
    } catch (error) { this.error = errorMessage(error); await this.client?.close();await this.sharingClient?.close(); await this.state?.close(); }
  }
  private persist(row: Row) {
    // Whitelist metadata. Never persist the seed, magnet link, source URLs or authorization.
    try { localStorage.setItem(HISTORY + row.id, JSON.stringify({ id:row.id, cacheId:row.cacheId, checkpointId:row.checkpointId,
      contentId:row.contentId, name:row.name, size:row.size, committed:row.committed, complete:row.complete,
      ...(rowOrigin(row) === 'magnet' ? { origin:'magnet' as const } : {}) })); }
    catch { this.historyError = true; }
  }
  async select(file: File, resumeId?: string) {
    if (!this.ready || this.closed || this.parsing) return;
    const generation = ++this.selecting; this.parsing = true; this.error = null; this.preview = null;this.sharePreview=null;
    try {
      const seed = await this.client.parseSeed(fileSource(file));
      if (this.closed || generation !== this.selecting) return;
      const row = resumeId ? this.rows.find(r => r.id === resumeId) : undefined;
      if (resumeId && (!row || row.contentId !== seed.info.id || row.size !== String(seed.info.size))) { this.error = 'mismatch'; return; }
      if (!seed.hints.contentSources && !this.networkEnabled && seed.info.size > 0n) { this.error = 'noSources'; return; }
      this.resumeId = resumeId; this.resumeOrigin = 'seed'; this.preview = seed;
    } catch (error) { if (!this.closed) this.error = error instanceof Q2QError && error.code === 'RESOURCE_LIMIT' ? 'invalid' : errorMessage(error); }
    finally { this.parsing = false; }
  }
  selectShare(file:File){
    if(!this.ready||this.closed)return;
    if(this.rows.length+this.shares.length>=32){this.error='tooMany';return;}
    this.error=null;this.preview=null;
    if(!this.sharingClient.capabilities.publish){this.error='shareUnsupported';return;}
    this.sharePreview=file;
  }
  /** Paste-to-resolve flow: parse locally first, then discover metadata.
   * `dn` from the link is deliberately never displayed or used for naming. */
  async openMagnet(link: string, resumeId?: string) {
    if (!this.ready || this.closed || this.magnetResolving) return;
    const generation = ++this.selecting;
    this.error = null; this.preview = null; this.sharePreview = null;
    this.magnetError = null; this.magnetLink = null; this.magnetResolving = true;
    this.magnetAbort = new AbortController();
    const signal = this.magnetAbort.signal;
    try {
      let parsed;
      try { parsed = await this.client.parseMagnet(link.trim(), { signal }); }
      catch (error) {
        if (this.closed || generation !== this.selecting) return;
        this.magnetError = error instanceof Q2QError && error.code === 'MAGNET_UNSUPPORTED' ? 'magnetUnsupported' : 'magnetInvalid';
        return;
      }
      if (this.closed || generation !== this.selecting || signal.aborted) return;
      this.magnetHash = parsed.infoHash;
      const seed = await this.client.resolveMagnet(parsed, { signal });
      if (this.closed || generation !== this.selecting || signal.aborted) return;
      const row = resumeId ? this.rows.find(r => r.id === resumeId) : undefined;
      if (resumeId && (!row || row.contentId !== seed.info.id || row.size !== String(seed.info.size))) { this.magnetError = 'mismatch'; return; }
      if (!resumeId && !seed.hints.contentSources && !this.networkEnabled && seed.info.size > 0n) { this.magnetError = 'noSources'; return; }
      this.resumeId = resumeId; this.resumeOrigin = 'magnet'; this.preview = seed;
    } catch (error) {
      if (this.closed || generation !== this.selecting) return;
      if (signal.aborted && !(error instanceof Q2QError)) return;
      if (error instanceof Q2QError && error.code === 'TASK_CANCELLED') return;
      this.magnetError = error instanceof Q2QError
        ? (error.code === 'UNSUPPORTED_CAPABILITY' ? 'magnetRequiresP2P'
          : error.code === 'NO_SOURCES' ? 'magnetNoSources'
          : error.code === 'CONTROL_TIMEOUT' ? 'magnetTimeout'
          : errorMessage(error))
        : 'network';
    } finally {
      this.magnetResolving = false; this.magnetHash = null; this.magnetAbort = undefined;
    }
  }
  cancelMagnet() {
    this.magnetAbort?.abort();
    this.magnetResolving = false; this.magnetHash = null; this.magnetError = null;
  }
  async startShare(){
    const file=this.sharePreview;
    if(!file||this.closed)return;
    this.sharePreview=null;
    try{
      this.addPublication(fileSource(file),filename(file.name));
    }catch(error){this.error=errorMessage(error);}
  }
  private addPublication(source:ContentSource,name:string,sourceCacheId?:string){
      if(this.rows.length+this.shares.length>=32)throw new Q2QError('RESOURCE_LIMIT');
      const handle=this.sharingClient.publish({source,name});
      this.shares=[{id:handle.id,name,size:String(source.info.size),snapshot:handle.snapshot(),sourceCacheId},...this.shares];
      const row=this.shares[0]!;this.publications.set(row.id,handle);
      const off=handle.subscribe(snapshot=>{row.snapshot=snapshot;if(snapshot.error)row.error=errorMessage(new Q2QError(snapshot.error.code));});
      void handle.ready().then(({seed})=>{this.shareSeeds.set(row.id,seed);row.seed=seed;}).catch(error=>{if(row.snapshot.state!=='closed')row.error=errorMessage(error);});
      // Closed handles retain only the bounded seed snapshot, never persist the File or private identity.
      handle.subscribe(snapshot=>{if(snapshot.state==='closed'||snapshot.state==='failed'){off();this.publications.delete(row.id);this.shareSeeds.delete(row.id);}});
  }
  async shareCompleted(row:Row){
    if(!row.complete||row.busy||this.closed)return;
    row.busy=true;try{const entry=await this.cache.open(row.cacheId);this.addPublication(entry.source,row.name,row.cacheId);}
    catch(error){row.error=errorMessage(error);}finally{row.busy=false;}
  }
  cacheSharing(row:Row){return this.shares.some(share=>share.sourceCacheId===row.cacheId&&!['closed','failed'].includes(share.snapshot.state));}
  async shareControl(row:ShareRow,action:'pause'|'resume'|'close'|'remove'){
    if(row.busy||this.closed)return;row.busy=true;row.error=undefined;
    try{
      const handle=this.publications.get(row.id);
      if(action==='remove'){await handle?.close();this.publications.delete(row.id);this.shares=this.shares.filter(item=>item.id!==row.id);}
      else await handle?.[action]();
    }catch(error){row.error=errorMessage(error);}finally{row.busy=false;}
  }
  saveSeed(row:ShareRow){
    const seed=this.shareSeeds.get(row.id);
    if(!seed||this.closed)return;
    const url=URL.createObjectURL(new Blob([new Uint8Array(seed.toBytes())],{type:'application/octet-stream'}));
    const link=document.createElement('a');link.href=url;link.download=`${row.name}.q2q`;document.body.append(link);link.click();link.remove();
    this.urls.set(url,setTimeout(()=>{URL.revokeObjectURL(url);this.urls.delete(url);},60000));
  }
  /** Share-side link generation is offline and pure; copy feedback is transient. */
  async copyMagnet(row:ShareRow){
    const seed=this.shareSeeds.get(row.id);
    if(!seed||row.busy||this.closed)return;
    row.busy=true;
    try{
      const link=await this.sharingClient.seedMagnet(seed);
      try{ await navigator.clipboard.writeText(link); }
      catch{ this.magnetLink=link; return; }
      row.magnetCopied=true;
      clearTimeout(this.copiedTimers.get(row.id));
      this.copiedTimers.set(row.id,setTimeout(()=>{row.magnetCopied=false;this.copiedTimers.delete(row.id);},2500));
    }catch(error){row.error=errorMessage(error);}
    finally{row.busy=false;}
  }
  sharingActive(){return this.shares.some(row=>!['closed','failed','paused'].includes(row.snapshot.state));}
  dismiss() { this.preview = null; this.resumeId = undefined; }
  async start() {
    const seed = this.preview;
    if (!seed || !this.ready || this.closed || this.starting) return;
    this.starting = true;
    this.preview = null; this.error = null;
    let row = this.resumeId ? this.rows.find(r => r.id === this.resumeId) : undefined;
    const origin = this.resumeOrigin; this.resumeOrigin = 'seed';
    this.resumeId = undefined;
    try {
      if (!row) {
        if (this.rows.length+this.shares.length >= 32) { this.error = 'tooMany'; return; }
        const entry = await this.cache.create({ name:filename(seed.info.name), size:seed.info.size });
        if (this.closed) { await this.cache.remove(entry.id); return; }
        row = { id:crypto.randomUUID(), cacheId:entry.id, checkpointId:null, contentId:seed.info.id,
          name:filename(seed.info.name), size:String(seed.info.size), committed:'0', complete:false,
          ...(origin === 'magnet' ? { origin:'magnet' as const } : {}) };
        this.rows = [row, ...this.rows];
        row = this.rows[0]!; this.persist(row);
      }
      await this.launch(row, seed);
    } catch (error) { if (row) row.error = errorMessage(error); else this.error = errorMessage(error); }
    finally { this.starting = false; }
  }
  private async launch(row: Row, seed: Seed) {
    if (this.closed) return;
    const old = this.handles.get(row.id);
    if (old && !['completed','cancelled','failed'].includes(old.snapshot().state)) await old.cancel();
    const entry = await this.cache.open(row.cacheId);
    const task = row.checkpointId
      ? this.client.resumeDownload({ seed, destination:entry.destination, checkpointId:row.checkpointId })
      : this.client.download({ seed, destination:entry.destination });
    row.error = undefined; row.unavailable = false; row.saved = false; row.submitted = false;
    this.handles.set(row.id, task); this.seeds.set(row.id, seed);
    const off = task.subscribe(snapshot => {
      // Even during close, retain the final checkpoint ID and committed hint.
      row.snapshot = snapshot; row.checkpointId = snapshot.checkpointId;
      row.committed = String(snapshot.committedBytes); row.complete = snapshot.state === 'completed';
      if (snapshot.error && snapshot.error.code !== 'TASK_CANCELLED') row.error = errorMessage(new Q2QError(snapshot.error.code));
      this.persist(row);
    });
    void task.wait().catch(() => {}).finally(off);
    if(this.networkEnabled&&this.client.capabilities.p2p){
      row.networkState='connecting';
      void this.client.connectNetwork().then(network=>{
        const unsubscribe=network.subscribe(snapshot=>{row.networkState=snapshot.state;});
        void task.wait().catch(()=>{}).finally(unsubscribe);
      }).catch(()=>{row.networkState='offline';});
    }
  }
  async control(row: Row, action: 'pause' | 'resume' | 'cancel' | 'retry') {
    if (row.busy || this.closed) return;
    row.busy = true; row.error = undefined;
    try {
      const task = this.handles.get(row.id);
      if (action === 'retry') { const seed = this.seeds.get(row.id); if (seed) await this.launch(row, seed); }
      else if (task) await task[action]();
    } catch (error) { row.error = errorMessage(error); }
    finally { row.busy = false; }
  }
  hasSeed(row: Row) { return this.seeds.has(row.id); }
  running(row: Row) { return !!row.snapshot && active.has(row.snapshot.state); }
  save(row: Row) {
    const work = this.saveEntry(row);
    this.exportWork.add(work);
    void work.finally(() => this.exportWork.delete(work)).catch(() => {});
    return work;
  }
  private async saveEntry(row: Row) {
    if (row.saving || row.busy || this.closed) return;
    row.error = undefined;
    const picker = (window as Window & { showSaveFilePicker?: (options: {suggestedName:string}) => Promise<FileSystemFileHandle> }).showSaveFilePicker;
    if (!picker && BigInt(row.size) > 32n * 1024n * 1024n) { row.error = 'unsupportedSave'; return; }
    row.saving = true;
    const controller = new AbortController(); this.exports.set(row.id, controller);
    try {
      // Invoke the native picker before any await, preserving user activation.
      if (!picker) {
        const entry = await this.cache.open(row.cacheId);
        const reader = await entry.source.open();
        const chunks: ArrayBuffer[] = [];
        try {
          for (let offset = 0n; offset < entry.size; offset += 1048576n) {
            if (controller.signal.aborted) return;
            const length = Number(entry.size - offset < 1048576n ? entry.size - offset : 1048576n);
            const bytes = await reader.read(offset, length, {signal:controller.signal});
            chunks.push(new Uint8Array(bytes).buffer);
            row.exportBytes = offset + BigInt(length);
          }
        } finally { await reader.close(); }
        if (controller.signal.aborted) return;
        const url = URL.createObjectURL(new Blob(chunks, {type:'application/octet-stream'}));
        const link = document.createElement('a'); link.href = url; link.download = row.name;
        document.body.append(link); link.click(); link.remove(); row.submitted = true;
        const timer = setTimeout(() => { URL.revokeObjectURL(url); this.urls.delete(url); }, 60000);
        this.urls.set(url,timer);
        return;
      }
      const handle = await picker.call(window, { suggestedName:row.name });
      if (controller.signal.aborted) return;
      const target = await fileDestination(handle, { overwrite: true });
      const entry = await this.cache.open(row.cacheId);
      const receipt = await entry.exportTo(target, { signal:controller.signal, onProgress: p => { row.exportBytes = p.copiedBytes; } });
      if (!controller.signal.aborted) row.saved = receipt.kind === 'user-file';
    } catch (error) {
      if (!controller.signal.aborted && !(error instanceof DOMException && error.name === 'AbortError')) row.error = errorMessage(error);
    } finally { row.saving = false; this.exports.delete(row.id); }
  }
  stopSave(row: Row) { this.exports.get(row.id)?.abort(); }
  async remove(row: Row) {
    if (row.busy || row.saving || this.closed) return;
    if(this.cacheSharing(row)){row.error='stopSharingFirst';return;}
    row.busy = true;
    try {
      await this.handles.get(row.id)?.cancel();
      // Remove bytes before metadata: on failure the row stays visible and retryable.
      if (!row.unavailable) {
        try { await this.cache.remove(row.cacheId); }
        catch (error) { if (!(error instanceof Q2QError && error.code === 'CACHE_NOT_FOUND')) throw error; }
        row.unavailable = true;
      }
      if (row.checkpointId) {
        try { await this.client.discardCheckpoint(row.checkpointId); }
        catch (error) { if (!(error instanceof Q2QError && error.code === 'CHECKPOINT_NOT_FOUND')) throw error; }
      }
      this.rows = this.rows.filter(r => r.id !== row.id); this.handles.delete(row.id); this.seeds.delete(row.id);
      try { localStorage.removeItem(HISTORY + row.id); } catch { this.historyError = true; }
    } catch (error) { row.error = errorMessage(error); }
    finally { row.busy = false; }
  }
  close() {
    if (this.closePromise) return this.closePromise;
    this.closed = true; this.selecting++;this.sharePreview=null;
    this.magnetAbort?.abort(); this.magnetResolving=false; this.magnetHash=null; this.magnetLink=null;
    for (const timer of this.copiedTimers.values()) clearTimeout(timer); this.copiedTimers.clear();
    for (const controller of this.exports.values()) controller.abort();
    this.closePromise = (async () => { try {
      const results = await Promise.allSettled([...this.exportWork, this.sharingClient?.close()??Promise.resolve(),this.client?.close() ?? Promise.resolve()]);
      const failed = results.find(result => result.status === 'rejected');
      if (failed?.status === 'rejected') throw failed.reason;
    } finally { await this.state?.close(); for (const [url,timer] of this.urls) { clearTimeout(timer); URL.revokeObjectURL(url); } this.urls.clear(); } })();
    return this.closePromise;
  }
}
