/** Public offsets are lossless; host adapters check their own numeric limits. */
export interface SourceInfo {
  readonly name: string;
  readonly size: bigint;
}

export interface ContentSource {
  readonly info: SourceInfo;
  /** Independent task lease. Closing it does not close other readers. */
  open(): Promise<SourceReader>;
}

export interface SourceReader {
  /** Exact range or error. End-of-file is not a successful short read. */
  read(offset: bigint, length: number, options?: { signal?: AbortSignal }): Promise<Uint8Array>;
  /** Waits for outstanding host reads; idempotent, rejects subsequent reads. */
  close(): Promise<void>;
}

/**
 * Storage contract shared by user-file and OPFS adapters.
 * Written bytes are not acknowledged as committed until commit succeeds. Adapters serialize
 * accepted writes/commit/finish and fence late work when aborting or closing.
 */
export interface ContentDestination {
  readonly targetId: string;
  /** Must exclusively lease the target; opening for resume must not truncate it. */
  open(options: { mode: 'create' | 'resume'; size: bigint; signal?: AbortSignal }): Promise<DestinationWriter>;
}

export interface DestinationWriter {
  /** Untrusted stored bytes for recovery, null if the range is not present. Never a sharing source. */
  read(offset: bigint, length: number): Promise<Uint8Array | null>;
  write(offset: bigint, bytes: Uint8Array): Promise<void>;
  /** Confirms host submission, not power-loss durability; no checkpoint is written here. */
  commit(): Promise<void>;
  /** Commits and closes, returning an honest description of the output location. */
  finish(): Promise<OutputReceipt>;
  /** Stops writes and releases the lease. Direct OPFS writes may remain and must be reverified; no pending progress is published. An already-started commit may complete. */
  abort(): Promise<void>;
}

export type OutputReceipt =
  | { readonly kind: 'user-file'; readonly targetId: string }
  | { readonly kind: 'staged'; readonly targetId: string; readonly needsExport: true };
