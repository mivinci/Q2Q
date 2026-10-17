CREATE TABLE project (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 80),
  personalUserId TEXT UNIQUE REFERENCES user(id) ON DELETE CASCADE,
  createdAt INTEGER NOT NULL
);

CREATE TABLE project_member (
  projectId TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK(role IN ('owner', 'member')),
  createdAt INTEGER NOT NULL,
  PRIMARY KEY (projectId, userId)
);
CREATE INDEX project_member_user_idx ON project_member(userId);
