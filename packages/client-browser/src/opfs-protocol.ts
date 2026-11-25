import type { StorageErrorCode } from './error.ts';

export type OpfsCommand =
  | { type: 'open'; path: readonly string[] }
  | { type: 'write'; offset: number; bytes: Uint8Array<ArrayBuffer> }
  | { type: 'read'; offset: number; length: number }
  | { type: 'flush' }
  | { type: 'close' };
export type OpfsRequest = OpfsCommand & { id: number };
export type OpfsResponse = { id: number; error?: StorageErrorCode; bytes?: Uint8Array<ArrayBuffer> | null };

// This API exists only in Dedicated Workers, so it is absent from lib.dom.
export interface SyncFile {
  write(bytes: Uint8Array<ArrayBuffer>, options: { at: number }): number;
  read(bytes: Uint8Array<ArrayBuffer>, options: { at: number }): number;
  getSize(): number;
  flush(): void;
  close(): void;
}
export interface SyncFileHandle extends FileSystemFileHandle {
  createSyncAccessHandle(): Promise<SyncFile>;
}
