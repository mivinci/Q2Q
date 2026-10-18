// Public task metadata only. Object keys, provider upload IDs and credentials stay on the server.
export const uploadPartSize = 8 * 1024 * 1024;
export const uploadMaxSize = 8 * 1024 * 1024 * 1024;
export type UploadStatus = 'initializing' | 'uploading' | 'completing' | 'uploaded' | 'cancelled' | 'failed';
export interface UploadSummary {
  id: string;
  fileId: string;
  name: string;
  size: number;
  status: UploadStatus;
  cleanupPending: number;
  canManage: number;
  createdAt: number;
  updatedAt: number;
}
export type { UploadedPart, PartGrant } from '@q2q/s3-storage';

export const uploadStatusLabels = {
  initializing: 'uploadInitializing', uploading: 'uploadInterrupted', completing: 'uploadCompleting',
  uploaded: 'uploadStored', cancelled: 'uploadCancelled', failed: 'uploadFailed'
} as const;

export function fileStatusLabel(status: string, uploadStatus?: UploadStatus | null, processingStatus?: import('./processing.js').ProcessingSummary['status'] | null) {
  if (status === 'uploaded' && processingStatus) return {
    pending: 'processingPending', running: 'processingRunning', ready: 'processingReady', failed: 'processingFailed'
  }[processingStatus] as 'processingPending' | 'processingRunning' | 'processingReady' | 'processingFailed';
  if (status === 'uploaded') return 'uploadAwaitingProcessing';
  if (status === 'deleting') return 'fileDeleting';
  return uploadStatus ? uploadStatusLabels[uploadStatus] : 'filePending';
}

export function formatBytes(bytes: number, locale: string): string {
  const unit = bytes >= 1024 ** 4 ? 'TiB' : bytes >= 1024 ** 3 ? 'GiB' : bytes >= 1024 ** 2 ? 'MiB' : bytes >= 1024 ? 'KiB' : 'B';
  const divisor = unit === 'TiB' ? 1024 ** 4 : unit === 'GiB' ? 1024 ** 3 : unit === 'MiB' ? 1024 ** 2 : unit === 'KiB' ? 1024 : 1;
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: unit === 'B' ? 0 : 1 }).format(bytes / divisor)} ${unit}`;
}
