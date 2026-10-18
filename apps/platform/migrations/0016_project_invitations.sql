CREATE TABLE project_invitation (
  id TEXT PRIMARY KEY NOT NULL,
  projectId TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  tokenHash TEXT NOT NULL UNIQUE,
  createdBy TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
  createdAt INTEGER NOT NULL,
  expiresAt INTEGER NOT NULL,
  acceptedBy TEXT REFERENCES user(id) ON DELETE SET NULL,
  acceptedAt INTEGER,
  revokedAt INTEGER
);

CREATE INDEX project_invitation_project_idx ON project_invitation(projectId, createdAt DESC);
