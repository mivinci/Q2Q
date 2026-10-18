export interface ProcessingSummary {
  status: 'pending' | 'running' | 'ready' | 'failed';
  processedBytes: number;
  errorCode: string | null;
  infoHash: string | null;
  leaseUntil: number | null;
  updatedAt: number;
}

export interface ProcessingMessage { fileId: string }
