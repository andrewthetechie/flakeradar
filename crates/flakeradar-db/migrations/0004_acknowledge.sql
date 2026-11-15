ALTER TABLE test_cases ADD COLUMN acknowledged_until timestamp with time zone;
ALTER TABLE test_cases ADD COLUMN acknowledge_reason text;
ALTER TABLE jobs ADD COLUMN acknowledged_until timestamp with time zone;
ALTER TABLE jobs ADD COLUMN acknowledge_reason text;
