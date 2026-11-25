CREATE TABLE digest_state (id smallint PRIMARY KEY CHECK (id = 1), last_sent_at timestamptz NOT NULL);
