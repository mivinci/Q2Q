export interface ContentIdentity {
  readonly id: `sha256:${string}`;
  readonly size: bigint;
  readonly blockSize: number;
}
export interface CheckpointBinding {
  readonly content: ContentIdentity;
  readonly targetId: string;
}
export interface Checkpoint extends CheckpointBinding {
  readonly id: string;
  readonly revision: number;
  /** A fresh bitmap copy. Persisted bits are hints until bytes are reverified. */
  readonly committed: Uint8Array;
  readonly committedBytes: bigint;
  readonly updatedAt: number;
}
export interface CheckpointLease {
  snapshot(): Checkpoint;
  readonly closed: boolean;
  /** Closes dependent IO when the state connection is invalidated. */
  readonly signal: AbortSignal;
  /** Optional local Merkle witnesses; untrusted until seed-root verification. */
  witness?(index:number):Promise<Uint8Array|null>;
  /** Witness updates and bitmap CAS must commit atomically. */
  replace(expectedRevision: number, committed: Uint8Array, witnesses?:ReadonlyMap<number,Uint8Array>): Promise<Checkpoint>;
  close(): Promise<void>;
}
export interface StateStore {
  create(binding: CheckpointBinding): Promise<Checkpoint>;
  read(id: string): Promise<Checkpoint>;
  list(options?: { after?: string; limit?: number }): Promise<{ items: readonly Checkpoint[]; next: string | null }>;
  lease(id: string): Promise<CheckpointLease>;
  /** Only removes metadata, never target bytes. Active checkpoints are busy. */
  discard(id: string, expectedRevision?: number): Promise<void>;
  close(): Promise<void>;
}
