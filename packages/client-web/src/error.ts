/** Codes are stable machine-readable values; UI applications translate them. */
export type StorageErrorCode =
  | 'INVALID_RANGE'
  | 'UNSUPPORTED_CAPABILITY'
  | 'SOURCE_CHANGED'
  | 'SOURCE_UNAVAILABLE'
  | 'PERMISSION_REQUIRED'
  | 'STORAGE_FULL'
  | 'STORAGE_UNAVAILABLE'
  | 'CACHE_NOT_FOUND'
  | 'CACHE_CORRUPT'
  | 'DESTINATION_EXISTS'
  | 'CHECKPOINT_MISMATCH'
  | 'CHECKPOINT_NOT_FOUND'
  | 'CHECKPOINT_CORRUPT'
  | 'CHECKPOINT_VERSION'
  | 'CHECKPOINT_BUSY'
  | 'CHECKPOINT_CONFLICT'
  | 'STATE_CLOSED'
  | 'INVALID_SEED'
  | 'CONTENT_MISMATCH'
  | 'RESOURCE_LIMIT'
  | 'DESTINATION_BUSY'
  | 'INVALID_STATE'
  | 'TASK_CANCELLED';

export type Q2QErrorCode = StorageErrorCode | 'CLIENT_CLOSED' | 'INVALID_ARGUMENT' | 'NO_SOURCES'
  | 'PEER_AUTH_FAILED' | 'PEER_PROTOCOL_ERROR' | 'PEER_CLOSED'
  | 'NETWORK_CLOSED' | 'NETWORK_PROTOCOL_ERROR' | 'PUNCH_UNAVAILABLE' | 'CONTROL_TIMEOUT'
  | 'PROOF_UNAVAILABLE' | 'INVALID_SOURCE' | 'NETWORK_UNAVAILABLE' | 'AUTH_REQUIRED'
  | 'HTTP_STATUS' | 'HTTP_PROTOCOL_ERROR' | 'HTTP_RANGE_UNSUPPORTED' | 'SOURCE_CHANGED';

export class Q2QError extends Error {
  readonly code: Q2QErrorCode;
  readonly retryable: boolean;
  constructor(code: Q2QErrorCode, retryable = false) {
    super(code);
    this.name = 'Q2QError';
    this.code = code;
    this.retryable = retryable;
  }
}
export class StorageError extends Q2QError {
  override readonly code: StorageErrorCode;
  constructor(code: StorageErrorCode) { super(code); this.code = code; this.name = 'StorageError'; }
}

export function storageFailure(error: unknown): StorageError {
  if (error instanceof StorageError) return error;
  const name = error instanceof Error ? error.name : '';
  if (name === 'NotAllowedError' || name === 'SecurityError') return new StorageError('PERMISSION_REQUIRED');
  if (name === 'DataCloneError' || name === 'NotSupportedError') return new StorageError('UNSUPPORTED_CAPABILITY');
  if (name === 'QuotaExceededError') return new StorageError('STORAGE_FULL');
  if (name === 'NoModificationAllowedError') return new StorageError('DESTINATION_BUSY');
  return new StorageError('STORAGE_UNAVAILABLE');
}
