-- FlakeRadar 2 baseline schema.
--
-- This is the recorded baseline schema (tests/parity/schema.sql) as
-- one applyable script: same tables, columns, types, defaults, constraint names
-- and index names. The migration in src/migration.rs runs it statement by
-- statement. Do not restate a semicolon inside a string literal: the splitter
-- splits on semicolons.

CREATE TABLE public.job_executions (
    id bigint NOT NULL,
    job_id integer NOT NULL,
    ci_job_id character varying(255) NOT NULL,
    ci_run_id character varying(255) DEFAULT ''::character varying NOT NULL,
    ci_run_attempt integer DEFAULT 1 NOT NULL,
    commit_sha character varying(64) NOT NULL,
    branch character varying(255) NOT NULL,
    status character varying(16) NOT NULL,
    url text DEFAULT ''::text NOT NULL,
    runner_name character varying(255) DEFAULT ''::character varying NOT NULL,
    runner_labels jsonb DEFAULT '[]'::jsonb NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL
);
CREATE SEQUENCE public.job_executions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.job_executions_id_seq OWNED BY public.job_executions.id;
CREATE TABLE public.job_score_history (
    job_id integer NOT NULL,
    day date NOT NULL,
    flakiness_score double precision NOT NULL,
    confirmed_flake_count integer DEFAULT 0 NOT NULL,
    executions integer DEFAULT 0 NOT NULL,
    failures integer DEFAULT 0 NOT NULL
);
CREATE TABLE public.jobs (
    id integer NOT NULL,
    pipeline_id integer NOT NULL,
    name character varying(512) NOT NULL,
    flakiness_score double precision DEFAULT '0'::double precision NOT NULL,
    confirmed_flake_count integer DEFAULT 0 NOT NULL,
    last_status character varying(16) DEFAULT 'passed'::character varying NOT NULL,
    last_seen_at timestamp with time zone NOT NULL,
    github_issue_number integer,
    created_at timestamp with time zone NOT NULL,
    clean_streak integer DEFAULT 0 NOT NULL,
    summary boolean DEFAULT false NOT NULL,
    github_issue_checked_at timestamp with time zone,
    github_issue_filed boolean,
    github_issue_closed boolean DEFAULT false NOT NULL,
    github_issue_fixed_noted_at timestamp with time zone
);
CREATE SEQUENCE public.jobs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.jobs_id_seq OWNED BY public.jobs.id;
CREATE TABLE public.pipelines (
    id integer NOT NULL,
    repo_id integer NOT NULL,
    provider character varying(32) NOT NULL,
    name character varying(512) NOT NULL,
    created_at timestamp with time zone NOT NULL
);
CREATE SEQUENCE public.pipelines_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.pipelines_id_seq OWNED BY public.pipelines.id;
CREATE TABLE public.projects (
    id integer NOT NULL,
    repo_id integer NOT NULL,
    name character varying(100) NOT NULL,
    root character varying(1024) DEFAULT ''::character varying NOT NULL,
    created_at timestamp with time zone NOT NULL
);
CREATE SEQUENCE public.projects_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.projects_id_seq OWNED BY public.projects.id;
CREATE TABLE public.reports (
    id integer NOT NULL,
    project_id integer,
    commit_sha character varying(64) NOT NULL,
    branch character varying(255) NOT NULL,
    ci_run_id character varying(255) DEFAULT ''::character varying NOT NULL,
    root character varying(1024),
    body bytea NOT NULL,
    status character varying(16) DEFAULT 'pending'::character varying NOT NULL,
    error text,
    counts jsonb,
    run_id integer,
    created_at timestamp with time zone NOT NULL,
    processed_at timestamp with time zone,
    kind character varying(16) DEFAULT 'junit'::character varying NOT NULL,
    repo_id integer NOT NULL,
    ci_job_id character varying(255),
    ci_run_attempt integer,
    pipeline character varying(512),
    default_branch character varying(255),
    retry_count integer DEFAULT 0 NOT NULL,
    body_sha256 character varying(64),
    line_base integer,
    CONSTRAINT ck_reports_junit_has_project CHECK ((((kind)::text <> 'junit'::text) OR (project_id IS NOT NULL)))
);
CREATE SEQUENCE public.reports_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.reports_id_seq OWNED BY public.reports.id;
CREATE TABLE public.repos (
    id integer NOT NULL,
    name character varying(255) NOT NULL,
    created_at timestamp with time zone NOT NULL,
    default_branch character varying(255)
);
CREATE SEQUENCE public.repos_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.repos_id_seq OWNED BY public.repos.id;
CREATE TABLE public.test_cases (
    id integer NOT NULL,
    project_id integer NOT NULL,
    fingerprint character varying(40) NOT NULL,
    suite text DEFAULT ''::text NOT NULL,
    classname text DEFAULT ''::text NOT NULL,
    name text NOT NULL,
    file text,
    line integer,
    flakiness_score double precision DEFAULT '0'::double precision NOT NULL,
    confirmed_flake_count integer DEFAULT 0 NOT NULL,
    last_status character varying(16) DEFAULT 'passed'::character varying NOT NULL,
    last_seen_at timestamp with time zone NOT NULL,
    quarantined boolean DEFAULT false NOT NULL,
    quarantined_at timestamp with time zone,
    github_issue_number integer,
    failure_category character varying(16),
    clean_streak integer DEFAULT 0 NOT NULL,
    github_issue_checked_at timestamp with time zone,
    quarantine_reason text,
    quarantine_expires_at timestamp with time zone,
    github_issue_filed boolean,
    fail_streak integer DEFAULT 0 NOT NULL,
    github_issue_closed boolean DEFAULT false NOT NULL,
    github_issue_fixed_noted_at timestamp with time zone
);
CREATE SEQUENCE public.test_cases_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.test_cases_id_seq OWNED BY public.test_cases.id;
CREATE TABLE public.test_executions (
    id bigint NOT NULL,
    test_case_id integer NOT NULL,
    test_run_id integer NOT NULL,
    status character varying(16) NOT NULL,
    duration double precision DEFAULT '0'::double precision NOT NULL,
    message text DEFAULT ''::text NOT NULL,
    details text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    attempt integer DEFAULT 0 NOT NULL,
    failure_category character varying(16)
);
CREATE SEQUENCE public.test_executions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.test_executions_id_seq OWNED BY public.test_executions.id;
CREATE TABLE public.test_runs (
    id integer NOT NULL,
    project_id integer NOT NULL,
    commit_sha character varying(64) NOT NULL,
    branch character varying(255) NOT NULL,
    ci_run_id character varying(255) DEFAULT ''::character varying NOT NULL,
    created_at timestamp with time zone NOT NULL,
    ci_job_id character varying(255),
    ci_run_attempt integer,
    pipeline character varying(512)
);
CREATE SEQUENCE public.test_runs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.test_runs_id_seq OWNED BY public.test_runs.id;
CREATE TABLE public.test_score_history (
    test_case_id integer NOT NULL,
    day date NOT NULL,
    flakiness_score double precision NOT NULL,
    confirmed_flake_count integer DEFAULT 0 NOT NULL,
    executions integer DEFAULT 0 NOT NULL,
    failures integer DEFAULT 0 NOT NULL
);
ALTER TABLE ONLY public.job_executions ALTER COLUMN id SET DEFAULT nextval('public.job_executions_id_seq'::regclass);
ALTER TABLE ONLY public.jobs ALTER COLUMN id SET DEFAULT nextval('public.jobs_id_seq'::regclass);
ALTER TABLE ONLY public.pipelines ALTER COLUMN id SET DEFAULT nextval('public.pipelines_id_seq'::regclass);
ALTER TABLE ONLY public.projects ALTER COLUMN id SET DEFAULT nextval('public.projects_id_seq'::regclass);
ALTER TABLE ONLY public.reports ALTER COLUMN id SET DEFAULT nextval('public.reports_id_seq'::regclass);
ALTER TABLE ONLY public.repos ALTER COLUMN id SET DEFAULT nextval('public.repos_id_seq'::regclass);
ALTER TABLE ONLY public.test_cases ALTER COLUMN id SET DEFAULT nextval('public.test_cases_id_seq'::regclass);
ALTER TABLE ONLY public.test_executions ALTER COLUMN id SET DEFAULT nextval('public.test_executions_id_seq'::regclass);
ALTER TABLE ONLY public.test_runs ALTER COLUMN id SET DEFAULT nextval('public.test_runs_id_seq'::regclass);
ALTER TABLE ONLY public.job_executions
    ADD CONSTRAINT job_executions_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.job_score_history
    ADD CONSTRAINT job_score_history_pkey PRIMARY KEY (job_id, day);
ALTER TABLE ONLY public.jobs
    ADD CONSTRAINT jobs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.pipelines
    ADD CONSTRAINT pipelines_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.repos
    ADD CONSTRAINT repos_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.test_cases
    ADD CONSTRAINT test_cases_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.test_executions
    ADD CONSTRAINT test_executions_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.test_runs
    ADD CONSTRAINT test_runs_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.test_score_history
    ADD CONSTRAINT test_score_history_pkey PRIMARY KEY (test_case_id, day);
ALTER TABLE ONLY public.job_executions
    ADD CONSTRAINT uq_job_executions_job_ci_job_id UNIQUE (job_id, ci_job_id);
ALTER TABLE ONLY public.jobs
    ADD CONSTRAINT uq_jobs_pipeline_name UNIQUE (pipeline_id, name);
ALTER TABLE ONLY public.pipelines
    ADD CONSTRAINT uq_pipelines_repo_provider_name UNIQUE (repo_id, provider, name);
ALTER TABLE ONLY public.projects
    ADD CONSTRAINT uq_projects_repo_name UNIQUE (repo_id, name);
ALTER TABLE ONLY public.repos
    ADD CONSTRAINT uq_repos_name UNIQUE (name);
ALTER TABLE ONLY public.test_cases
    ADD CONSTRAINT uq_test_cases_project_fingerprint UNIQUE (project_id, fingerprint);
CREATE INDEX ix_exec_case_id ON public.test_executions USING btree (test_case_id, id);
CREATE INDEX ix_job_exec_job_id ON public.job_executions USING btree (job_id, id);
CREATE INDEX ix_job_executions_ci_job_id ON public.job_executions USING btree (ci_job_id);
CREATE INDEX ix_job_executions_ci_run_id ON public.job_executions USING btree (ci_run_id);
CREATE INDEX ix_job_executions_created_at ON public.job_executions USING btree (created_at);
CREATE INDEX ix_job_score_history_day ON public.job_score_history USING btree (day);
CREATE INDEX ix_jobs_issue_check ON public.jobs USING btree (github_issue_checked_at NULLS FIRST, id) WHERE (github_issue_number IS NOT NULL);
CREATE INDEX ix_jobs_pipeline_id ON public.jobs USING btree (pipeline_id);
CREATE INDEX ix_jobs_pipeline_score ON public.jobs USING btree (pipeline_id, flakiness_score);
CREATE INDEX ix_pipelines_repo_id ON public.pipelines USING btree (repo_id);
CREATE INDEX ix_projects_repo_id ON public.projects USING btree (repo_id);
CREATE INDEX ix_reports_project_body_sha256 ON public.reports USING btree (project_id, body_sha256);
CREATE INDEX ix_reports_project_id ON public.reports USING btree (project_id);
CREATE INDEX ix_reports_repo_ci_run_id ON public.reports USING btree (repo_id, ci_run_id);
CREATE INDEX ix_reports_repo_id ON public.reports USING btree (repo_id);
CREATE INDEX ix_reports_status_id ON public.reports USING btree (status, id);
CREATE INDEX ix_test_cases_issue_check ON public.test_cases USING btree (github_issue_checked_at NULLS FIRST, id) WHERE (github_issue_number IS NOT NULL);
CREATE INDEX ix_test_cases_project_id ON public.test_cases USING btree (project_id);
CREATE INDEX ix_test_cases_project_score ON public.test_cases USING btree (project_id, flakiness_score);
CREATE INDEX ix_test_executions_created_at ON public.test_executions USING btree (created_at);
CREATE INDEX ix_test_executions_test_run_id ON public.test_executions USING btree (test_run_id);
CREATE INDEX ix_test_runs_ci_job_id ON public.test_runs USING btree (ci_job_id);
CREATE INDEX ix_test_runs_ci_run_id ON public.test_runs USING btree (ci_run_id);
CREATE INDEX ix_test_runs_commit_sha ON public.test_runs USING btree (commit_sha);
CREATE INDEX ix_test_runs_project_id ON public.test_runs USING btree (project_id);
CREATE INDEX ix_test_score_history_day ON public.test_score_history USING btree (day);
ALTER TABLE ONLY public.job_executions
    ADD CONSTRAINT job_executions_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.job_score_history
    ADD CONSTRAINT job_score_history_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.jobs
    ADD CONSTRAINT jobs_pipeline_id_fkey FOREIGN KEY (pipeline_id) REFERENCES public.pipelines(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.pipelines
    ADD CONSTRAINT pipelines_repo_id_fkey FOREIGN KEY (repo_id) REFERENCES public.repos(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_repo_id_fkey FOREIGN KEY (repo_id) REFERENCES public.repos(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_repo_id_fkey FOREIGN KEY (repo_id) REFERENCES public.repos(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.test_runs(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.test_cases
    ADD CONSTRAINT test_cases_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.test_executions
    ADD CONSTRAINT test_executions_test_case_id_fkey FOREIGN KEY (test_case_id) REFERENCES public.test_cases(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.test_executions
    ADD CONSTRAINT test_executions_test_run_id_fkey FOREIGN KEY (test_run_id) REFERENCES public.test_runs(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.test_runs
    ADD CONSTRAINT test_runs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;
ALTER TABLE ONLY public.test_score_history
    ADD CONSTRAINT test_score_history_test_case_id_fkey FOREIGN KEY (test_case_id) REFERENCES public.test_cases(id) ON DELETE CASCADE;
