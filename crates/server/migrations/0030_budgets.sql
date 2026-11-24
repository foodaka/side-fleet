-- Agent budgets: a run may pay for APIs over MPP, up to budget_micros (millionths of a
-- dollar), asking for approval above ask_above_micros. The worker makes an access key for
-- the run and offers its address (pay_key); the user's root key signs an authorization for
-- it (pay_authorization). Both are public data: no private key is ever stored here.
ALTER TABLE runs ADD COLUMN budget_micros bigint;
ALTER TABLE runs ADD COLUMN ask_above_micros bigint;
ALTER TABLE runs ADD COLUMN spent_micros bigint NOT NULL DEFAULT 0;
ALTER TABLE runs ADD COLUMN pay_key jsonb;
ALTER TABLE runs ADD COLUMN pay_authorization jsonb;

-- Which services a task's agent may pay without asking: your approved services (and whether
-- anything else asks), one per person, or per org for the developer key. Copied onto each run
-- with a budget (runs.pay_policy), so a task keeps the list it started with.
ALTER TABLE runs ADD COLUMN pay_policy jsonb;
CREATE TABLE pay_policies (
    org_id uuid NOT NULL,
    user_id uuid,
    policy jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX pay_policies_owner ON pay_policies (org_id, COALESCE(user_id, '00000000-0000-0000-0000-000000000000'));
