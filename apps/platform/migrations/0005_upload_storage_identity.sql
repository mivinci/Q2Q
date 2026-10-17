-- Freeze the data destination so a configuration change cannot mix local and remote multipart IDs.
ALTER TABLE upload_task ADD COLUMN transport TEXT NOT NULL DEFAULT 'local' CHECK(transport IN ('local', 's3'));
ALTER TABLE upload_task ADD COLUMN bucketIdentity TEXT NOT NULL DEFAULT 'local-r2';
