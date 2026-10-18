ALTER TABLE seed_task ADD COLUMN seedDigest TEXT;
ALTER TABLE seed_task ADD COLUMN seedSize INTEGER;
-- Existing pre-release artifacts must be regenerated before authenticated download.
UPDATE seed_task SET status = 'failed', errorCode = 'processingFailed' WHERE status = 'ready';
