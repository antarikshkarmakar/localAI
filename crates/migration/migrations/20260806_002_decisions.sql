-- Council decisions (spec 05 §6). Every COUNCIL_* call lands one row: the
-- synthesized verdict PLUS every member's individual vote, so dissent is
-- preserved and never averaged away (C5), and the diversity flag (C12) is
-- queryable for the anti-collusion audit (G-04).
CREATE TABLE decisions (
    id             INTEGER PRIMARY KEY,
    event_id       INTEGER NOT NULL REFERENCES events(id),  -- the COUNCIL_* route event
    mode           TEXT    NOT NULL,   -- 'decide'|'security'|'fact'|'audit'
    question       TEXT    NOT NULL,
    chair          TEXT,               -- provider that synthesized (rotating, C4)
    verdict        TEXT    NOT NULL,   -- synthesized outcome
    votes_json     TEXT    NOT NULL,   -- per-member: stance, confidence, citation, dissent
    diversity_flag TEXT,               -- 'low_diversity' when unanimous on a contested claim (C12)
    cost_usd       REAL,
    created        TEXT    NOT NULL
);
CREATE INDEX idx_decisions_event ON decisions(event_id);
CREATE INDEX idx_decisions_mode  ON decisions(mode);

-- Per-provider running spend, for the C15 daily/monthly ceiling check.
-- One row per (provider, day); the monthly figure is a SUM over the month.
CREATE TABLE council_spend (
    provider  TEXT NOT NULL,
    day       TEXT NOT NULL,           -- 'YYYY-MM-DD' (caller-injected, G-09)
    calls     INTEGER NOT NULL DEFAULT 0,
    cost_usd  REAL    NOT NULL DEFAULT 0.0,
    PRIMARY KEY (provider, day)
);
