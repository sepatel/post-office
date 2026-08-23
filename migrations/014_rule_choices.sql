-- Collapses `response_mode` and `label_selection` into one menu the model picks
-- from. A choice may be a label or an action, so "file it under A, B, or bin it"
-- becomes expressible instead of something the prompt had to argue the model out
-- of. `actions` keeps its original meaning: the recipe that runs on any match.
--
-- The backfill preserves prior behavior:
--   legacy_token          -> no menu; APPLY/SKIP was exactly match-or-not over
--                            the configured actions
--   decision + none       -> no menu
--   decision + configured -> the label actions become the menu and leave
--                            `actions`, so non-label actions still always run
--   decision + all_user_labels -> choose_from_all_labels

ALTER TABLE rules ADD COLUMN choices TEXT NOT NULL DEFAULT '[]';
ALTER TABLE rules ADD COLUMN choose_from_all_labels INTEGER NOT NULL DEFAULT 0;

UPDATE rules
SET choose_from_all_labels = 1
WHERE response_mode = 'decision' AND label_selection = 'all_user_labels';

UPDATE rules
SET choices = (
        SELECT COALESCE(json_group_array(json(action.value)), '[]')
        FROM json_each(rules.actions) AS action
        WHERE json_extract(action.value, '$.type') = 'label'
    ),
    actions = (
        SELECT COALESCE(json_group_array(json(action.value)), '[]')
        FROM json_each(rules.actions) AS action
        WHERE json_extract(action.value, '$.type') <> 'label'
    )
WHERE response_mode = 'decision' AND label_selection = 'configured';

ALTER TABLE rules DROP COLUMN response_mode;
ALTER TABLE rules DROP COLUMN label_selection;
