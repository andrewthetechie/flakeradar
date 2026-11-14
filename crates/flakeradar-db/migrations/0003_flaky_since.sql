ALTER TABLE test_cases ADD COLUMN flaky_since timestamp with time zone;
ALTER TABLE jobs ADD COLUMN flaky_since timestamp with time zone;
UPDATE test_cases SET flaky_since = now() WHERE flakiness_score >= 0.30;
UPDATE jobs SET flaky_since = now() WHERE flakiness_score >= 0.30;
