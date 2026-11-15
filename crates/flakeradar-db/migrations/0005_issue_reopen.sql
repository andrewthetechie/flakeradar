ALTER TABLE test_cases ADD COLUMN github_issue_last_closed_number integer;
ALTER TABLE test_cases ADD COLUMN github_issue_last_closed_at timestamp with time zone;
ALTER TABLE jobs ADD COLUMN github_issue_last_closed_number integer;
ALTER TABLE jobs ADD COLUMN github_issue_last_closed_at timestamp with time zone;
