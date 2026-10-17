import { env } from 'cloudflare:workers';
import { error, redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';
import { requireFile } from '#lib/server/files.js';
import { listUploads } from '#lib/server/uploads.js';
import { getPlatformStorage } from '#lib/server/storage.js';
import { uploadTransport } from '#lib/server/upload-storage.js';
import { processingSummary } from '#lib/server/seed-tasks.js';
import { publicationBlocksDeletion, listPublications } from '#lib/server/publications.js';
import { projectNetwork } from '#lib/server/network.js';

export const load: PageServerLoad = async ({ locals, params, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject } = await parent();
  const record = await requireFile(params.id, locals.user.id);
  // A valid membership in another project must not mix its data into the selected workspace.
  if (!currentProject || record.projectId !== currentProject.id) error(404, 'fileNotFound');
  const connection = await getPlatformStorage(env.DB, currentProject.id);
  const transport = uploadTransport();
  return {
    network: await projectNetwork(record.projectId),
    publications: await listPublications(record.id, locals.user.id),
    publicationDeletionBlocked: await publicationBlocksDeletion(record.id),
    processingAvailable: transport === 's3' && !!env.PROCESSING_QUEUE,
    processing: await processingSummary(env.DB, record.id),
    file: { id: record.id, name: record.name, size: record.size, status: record.status, createdAt: record.createdAt, updatedAt: record.updatedAt },
    canManage: record.createdBy === locals.user.id || currentProject.role === 'owner',
    uploads: await listUploads(currentProject.id, locals.user.id, record.id),
    uploadReady: !!connection && !!transport, storageEnabled: !!connection, localUploads: transport === 'local'
  };
};
