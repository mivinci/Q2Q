export { fileSource } from './file-source.ts';
export { StorageError } from './error.ts';
export type { StorageErrorCode } from './error.ts';
export type {
  ContentSource, SourceInfo, SourceReader,
  ContentDestination, DestinationWriter, OutputReceipt
} from './storage.ts';

export { fileDestination } from './file-destination.ts';
export type { FileDestinationOptions } from './file-destination.ts';

export { opfsCache } from './opfs-cache.ts';
export type { OpfsCache, OpfsCacheOptions, OpfsEntry, ExportOptions } from './opfs-cache.ts';

export { browserState } from './browser-state.ts';
export type { ContentIdentity, CheckpointBinding, Checkpoint, CheckpointLease, StateStore } from './checkpoint-types.ts';
export { createBlockVerifier } from './block-verifier.ts';
export type { ContentVerifier } from './block-verifier.ts';
export { resumeCheckpoint } from './resume-checkpoint.ts';
export type { CheckpointSession } from './resume-checkpoint.ts';

export { createClient } from './client.ts';
export { Q2QError } from './error.ts';
export type { Q2QErrorCode, ConnectionDiagnostic } from './error.ts';
export type { Client, ClientOptions, ClientLimits, Seed, ContentInfo, SourceProvider, HttpSources, MagnetLink, DownloadOptions, ResumeDownloadOptions, Transfer, DownloadState, DownloadSnapshot, DownloadResult, Failure } from './client-types.ts';

export type { NetworkSession, NetworkSnapshot } from './network.ts';

export type { Publication, PublicationSnapshot, PublicationState, PublishOptions } from './client-types.ts';
