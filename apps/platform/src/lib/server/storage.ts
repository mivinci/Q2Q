export interface StorageConnection {
  id: string;
  projectId: string;
  provider: 'platform-s3';
  createdAt: number;
}

export async function getPlatformStorage(db: D1Database, projectId: string) {
  return db.prepare(`SELECT id, projectId, provider, createdAt
    FROM storage_connection WHERE projectId = ? AND provider = 'platform-s3'`)
    .bind(projectId).first<StorageConnection>();
}

export async function enablePlatformStorage(db: D1Database, projectId: string): Promise<void> {
  // Membership and owner checks happen before this call; uniqueness makes retries safe.
  await db.prepare(`INSERT INTO storage_connection (id, projectId, provider, createdAt)
    VALUES (?, ?, 'platform-s3', ?) ON CONFLICT(projectId, provider) DO NOTHING`)
    .bind(crypto.randomUUID(), projectId, Date.now()).run();
}
