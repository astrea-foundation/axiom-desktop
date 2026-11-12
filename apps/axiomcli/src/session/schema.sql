-- Native account-scoped storage baseline (AXA3, schema 1).
CREATE TABLE threads(
  id TEXT PRIMARY KEY,
  title TEXT,
  cwd BLOB NOT NULL,
  cwd_encoding TEXT NOT NULL,
  origin TEXT NOT NULL,
  profile TEXT NOT NULL,
  selected_model TEXT,
  thinking_level TEXT NOT NULL DEFAULT 'medium',
  lifecycle TEXT NOT NULL DEFAULT 'ready',
  archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
  revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
  next_timeline_sequence INTEGER NOT NULL DEFAULT 0 CHECK(next_timeline_sequence >= 0),
  last_message_at TEXT,
  last_user_message_at TEXT,
  last_reported_usage TEXT,
  desktop_agent_settings TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_threads_activity ON threads(archived, updated_at DESC, id);
CREATE INDEX idx_threads_messages ON threads(archived, COALESCE(last_message_at,'') DESC, id);
CREATE INDEX idx_threads_submissions ON threads(archived, COALESCE(last_user_message_at,'') DESC, id);

CREATE TABLE turns(
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  status TEXT NOT NULL,
  user_item_id TEXT,
  assistant_item_id TEXT,
  input_tokens INTEGER NOT NULL DEFAULT 0 CHECK(input_tokens >= 0),
  output_tokens INTEGER NOT NULL DEFAULT 0 CHECK(output_tokens >= 0),
  error TEXT,
  started_at TEXT NOT NULL,
  completed_at TEXT,
  PRIMARY KEY(thread_id, id)
);
CREATE INDEX idx_turns_thread_started ON turns(thread_id, started_at, id);

CREATE TABLE timeline_items(
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  turn_id TEXT,
  sequence INTEGER NOT NULL CHECK(sequence > 0),
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  client_item_id TEXT,
  external_id TEXT,
  content TEXT NOT NULL DEFAULT '',
  metadata TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(thread_id, sequence),
  UNIQUE(thread_id, client_item_id)
);
CREATE INDEX idx_timeline_turn ON timeline_items(thread_id, turn_id, sequence);
CREATE INDEX idx_timeline_external ON timeline_items(thread_id, external_id);

CREATE TABLE collections(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  collapsed INTEGER NOT NULL DEFAULT 0 CHECK(collapsed IN (0,1)),
  position INTEGER NOT NULL CHECK(position >= 0),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_collections_position ON collections(position, id);

CREATE TABLE thread_collections(
  thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
  collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  assigned_at TEXT NOT NULL
);
CREATE INDEX idx_thread_collections_collection ON thread_collections(collection_id, thread_id);

CREATE TABLE profile_preferences(
  id INTEGER PRIMARY KEY CHECK(id=1),
  selected_model TEXT,
  thinking_level TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO profile_preferences(id, selected_model, thinking_level, updated_at)
VALUES(1, NULL, 'medium', strftime('%Y-%m-%dT%H:%M:%fZ','now'));

CREATE TABLE collection_state(
  id INTEGER PRIMARY KEY CHECK(id=1),
  revision INTEGER NOT NULL CHECK(revision >= 0)
);
INSERT INTO collection_state(id, revision) VALUES(1, 0);

CREATE TABLE request_usage (
  request_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  record TEXT NOT NULL
);
CREATE INDEX request_usage_thread ON request_usage(thread_id);

PRAGMA application_id=1096302899;
PRAGMA user_version=1;
