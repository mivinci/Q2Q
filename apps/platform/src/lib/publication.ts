export interface PublicationSummary {
  id: string;
  projectId: string;
  fileId: string;
  status: 'creating' | 'published' | 'failed' | 'withdrawn';
  name: string;
  size: number;
  infoHash: string;
  createdAt: number;
  updatedAt: number;
  withdrawnAt: number | null;
  cleanupPending: number;
}
