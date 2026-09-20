ALTER TABLE workflow_llm_attempts ADD COLUMN provider_name TEXT;

UPDATE workflow_runs
SET last_error = NULL
WHERE state IN ('completed', 'resolved_externally');

UPDATE workflow_llm_attempts
SET provider_id = 'legacy',
    provider_name = COALESCE((SELECT value FROM config WHERE key = 'llm.legacy_name'), 'Legacy provider'),
    model = COALESCE((SELECT value FROM config WHERE key = 'llm.default_model'), 'unknown'),
    endpoint = COALESCE((SELECT value FROM config WHERE key = 'llm.base_url'), endpoint)
WHERE provider_id IS NULL
  AND error LIKE '%legacy (%';

UPDATE workflow_llm_attempts AS attempt
SET provider_id = (
        SELECT json_extract(profile.value, '$.id')
        FROM config, json_each(config.value) AS profile
        WHERE config.key = 'llm.providers'
          AND attempt.error LIKE '%' || json_extract(profile.value, '$.id') || ' (%'
        LIMIT 1
    ),
    provider_name = (
        SELECT json_extract(profile.value, '$.name')
        FROM config, json_each(config.value) AS profile
        WHERE config.key = 'llm.providers'
          AND attempt.error LIKE '%' || json_extract(profile.value, '$.id') || ' (%'
        LIMIT 1
    ),
    model = (
        SELECT json_extract(profile.value, '$.model')
        FROM config, json_each(config.value) AS profile
        WHERE config.key = 'llm.providers'
          AND attempt.error LIKE '%' || json_extract(profile.value, '$.id') || ' (%'
        LIMIT 1
    ),
    endpoint = (
        SELECT json_extract(profile.value, '$.base_url')
        FROM config, json_each(config.value) AS profile
        WHERE config.key = 'llm.providers'
          AND attempt.error LIKE '%' || json_extract(profile.value, '$.id') || ' (%'
        LIMIT 1
    )
WHERE attempt.provider_id IS NULL
  AND EXISTS (
      SELECT 1
      FROM config, json_each(config.value) AS profile
      WHERE config.key = 'llm.providers'
        AND attempt.error LIKE '%' || json_extract(profile.value, '$.id') || ' (%'
  );
