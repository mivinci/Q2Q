import { env } from 'cloudflare:workers';
import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';
import { listFiles } from '#lib/server/files.js';
import { planMetadata } from '#lib/server/plans.js';
import { getPlatformStorage } from '#lib/server/storage.js';
import { getStorageUsage } from '#lib/server/storage-quota.js';
import { uploadTransport } from '#lib/server/upload-storage.js';

export const load: PageServerLoad = async ({ locals, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject, entitlements } = await parent();
  if (!currentProject || !entitlements) redirect(303, '/login');

  const transport = uploadTransport();
  const [files, fileCount, storageUsage, storageConnection] = await Promise.all([
    listFiles(currentProject.id, locals.user.id),
    env.DB.prepare("SELECT COUNT(*) AS count FROM file_record WHERE projectId = ? AND status <> 'deleted'")
      .bind(currentProject.id).first<{ count: number }>(),
    getStorageUsage(env.DB, currentProject.id),
    getPlatformStorage(env.DB, currentProject.id)
  ]);

  return {
    currentProject,
    entitlements,
    planNameKey: planMetadata[entitlements.planId].nameKey,
    fileCount: fileCount?.count ?? 0,
    recentFiles: files.slice(0, 5),
    storageUsage,
    storageEnabled: !!storageConnection,
    uploadReady: !!storageConnection && !!transport
  };
};
