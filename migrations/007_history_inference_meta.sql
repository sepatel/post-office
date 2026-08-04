CREATE TABLE IF NOT EXISTS history_inference_meta (
    history_id  INTEGER PRIMARY KEY REFERENCES history(id) ON DELETE CASCADE,
    provider_id TEXT,
    policy_id   TEXT
);
