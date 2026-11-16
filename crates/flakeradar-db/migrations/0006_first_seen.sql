ALTER TABLE test_cases ADD COLUMN first_seen_at timestamp with time zone;
UPDATE test_cases t SET first_seen_at = COALESCE((SELECT min(e.created_at) FROM test_executions e WHERE e.test_case_id = t.id), t.last_seen_at);
ALTER TABLE test_cases ALTER COLUMN first_seen_at SET NOT NULL;
