-- Replaces the allow_label_selection boolean with a tri-state so a rule can
-- scope the model's label menu to its own configured labels ('configured'),
-- which is how a rule expresses "classify into exactly one of these".
-- The backfill preserves prior behavior: the boolean only ever meant
-- "offer every user label".
ALTER TABLE rules ADD COLUMN label_selection TEXT NOT NULL DEFAULT 'none';

UPDATE rules
SET label_selection = CASE WHEN allow_label_selection = 1 THEN 'all_user_labels' ELSE 'none' END;

ALTER TABLE rules DROP COLUMN allow_label_selection;
