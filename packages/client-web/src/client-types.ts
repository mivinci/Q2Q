import type { ContentDestination, ContentSource, OutputReceipt } from './storage.ts';
import type { ContentIdentity, StateStore } from './checkpoint-types.ts';
import type { Q2QErrorCode, ConnectionDiagnostic } from './error.ts';

export interface ContentInfo extends ContentIdentity { readonly name: string | null; readonly blockCount: bigint }
export interface Seed {
  readonly info: ContentInfo;
  readonly hints: Readonly<{ contentSources: number; proofSources: number; tracks: number }>;
  toBytes(): Uint8Array;
}
export interface HttpSources { readonly contentUrls: readonly string[]; readonly proofUrls?: readonly string[] }
/** Parsed magnet link: `infoHash` is the content identity; name and trackers are hints only. */
export interface MagnetLink { readonly infoHash: string; readonly displayName: string | null; readonly trackers: readonly string[] }
export interface SourceProvider {
  resolve(options: { content: ContentInfo; reason: 'initial' | 'unauthorized'; signal: AbortSignal }): Promise<HttpSources>;
}
export interface ClientLimits {
  httpRequests: number;
  peerConnections: number;
  uploadConnections: number;
  uploadReads: number;
  maxTasks: number;
  commitBytes: number;
  requestTimeoutMs: number;
  attemptsPerSource: number;
  retryDelayMs: number;
}
export interface ClientOptions { state?: StateStore; sourceProvider?: SourceProvider; limits?: Partial<ClientLimits>; trackUrl?: string; wasmUrl?: string | URL }
export interface DownloadOptions { seed: Seed; destination: ContentDestination }
/** Seek-driven window read: fetch only what the cursor needs, ahead by a bounded prefetch. */
export interface StreamOptions {
  seed: Seed;
  destination: ContentDestination;
  /** Blocks fetched ahead of the cursor (default 8, maximum 64; 0 fetches only the current block). */
  prefetchBlocks?: number;
  /** Maximum wait for one missing block, including verification and disk IO (default 30s). */
  readTimeoutMs?: number;
  /** Resume a stream this client created; the checkpoint must match the seed. */
  checkpointId?: string;
}
export interface StreamReader {
  readonly info: ContentInfo;
  readonly position: bigint;
  readonly checkpointId: string;
  /** Waits for the block under the cursor, then returns verified bytes. A read
   * never crosses a block boundary and may return fewer than `length` bytes. */
  read(length: number, options?: { signal?: AbortSignal }): Promise<Uint8Array>;
  seek(offset: bigint): Promise<void>;
  progress(): Readonly<DownloadSnapshot>;
  /** Cancels network work; verified blocks and the checkpoint stay resumable. */
  close(): Promise<void>;
}
export interface ResumeDownloadOptions extends DownloadOptions { checkpointId: string }
export type DownloadState = 'preparing' | 'waiting' | 'downloading' | 'pausing' | 'paused'
  | 'finalizing' | 'completed' | 'cancelling' | 'cancelled' | 'failed';
export interface Failure { readonly code: Q2QErrorCode; readonly retryable: boolean; readonly diagnostic?: Readonly<ConnectionDiagnostic> }
export interface DownloadSnapshot {
  readonly id: string; readonly state: DownloadState; readonly content: ContentInfo;
  readonly receivedBytes: bigint; readonly verifiedBytes: bigint; readonly committedBytes: bigint;
  readonly bytesPerSecond: number; readonly checkpointId: string | null; readonly error: Failure | null;
}
export interface DownloadResult { readonly content: ContentInfo; readonly output: OutputReceipt }
export interface Transfer {
  readonly id: string;
  snapshot(): Readonly<DownloadSnapshot>;
  subscribe(listener: (snapshot: Readonly<DownloadSnapshot>) => void): () => void;
  pause(): Promise<void>;
  resume(): Promise<void>;
  cancel(): Promise<void>;
  wait(): Promise<DownloadResult>;
}
export interface PublishOptions { source: ContentSource; name?: string }
export type PublicationState = 'preparing'|'publishing'|'pausing'|'paused'|'closing'|'closed'|'failed';
export interface PublicationSnapshot {
  readonly id:string;readonly state:PublicationState;readonly name:string|null;readonly totalBytes:bigint;
  readonly contentId:string|null;readonly preparedBytes:bigint;readonly sentBytes:bigint;
  readonly bytesPerSecond:number;readonly activeConnections:number;readonly error:Failure|null;
}
export interface Publication {
  readonly id:string;
  snapshot():Readonly<PublicationSnapshot>;
  subscribe(listener:(snapshot:Readonly<PublicationSnapshot>)=>void):()=>void;
  ready():Promise<{readonly seed:Seed}>;
  pause():Promise<void>;
  resume():Promise<void>;
  close():Promise<void>;
}
export interface Client {
  readonly capabilities: Readonly<{ http: true; p2p: boolean; publish: boolean; checkpointStore: 'memory' | 'provided' }>;
  connectNetwork(): Promise<import('./network.ts').NetworkSession>;
  parseSeed(input: Uint8Array | ContentSource, options?: { signal?: AbortSignal }): Promise<Seed>;
  /** Pure local magnet parse via the shared Wasm implementation; never touches the network. */
  parseMagnet(link: string, options?: { signal?: AbortSignal }): Promise<MagnetLink>;
  /** Cache-first metadata discovery. A cache hit works without P2P; a miss needs an online peer. */
  resolveMagnet(link: string | MagnetLink, options?: { signal?: AbortSignal }): Promise<Seed>;
  /** Offline magnet link generation from a local seed handle. */
  seedMagnet(seed: Seed): Promise<string>;
  publish(options: PublishOptions): Publication;
  /** Sharing ends before the download pauses, cancels, fails or finalizes. */
  downloadShared(options: DownloadOptions): {readonly transfer:Transfer;readonly publication:Publication};
  resumeDownloadShared(options: ResumeDownloadOptions): {readonly transfer:Transfer;readonly publication:Publication};
  download(options: DownloadOptions): Transfer;
  /** Opens a seek-driven reader; requires a proof table (peer-only content is
   * rejected) and a target whose committed ranges can be read back. */
  openStream(options: StreamOptions): Promise<StreamReader>;
  resumeDownload(options: ResumeDownloadOptions): Transfer;
  listCheckpoints(options?: { after?: string; limit?: number }): ReturnType<StateStore['list']>;
  discardCheckpoint(id: string): Promise<void>;
  close(): Promise<void>;
}
