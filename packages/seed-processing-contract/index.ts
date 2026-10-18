export interface ProcessingSummary {
  status: 'pending' | 'running' | 'ready' | 'failed';
  processedBytes: number;
  errorCode: string | null;
  infoHash: string | null;
  leaseUntil: number | null;
  updatedAt: number;
}

export interface SeedProcessingMessage { kind?: undefined; fileId: string }
export interface PublicationProcessingMessage { kind: 'publication'; fileId: string; publicationId: string }
export interface PublicationCleanupMessage { kind: 'publication-cleanup'; fileId: string; publicationId: string }
export type ProcessingMessage = SeedProcessingMessage | PublicationProcessingMessage | PublicationCleanupMessage;
