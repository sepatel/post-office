ALTER TABLE rules ADD COLUMN response_mode TEXT NOT NULL DEFAULT 'legacy_token';
ALTER TABLE rules ADD COLUMN allow_label_selection INTEGER NOT NULL DEFAULT 0;
