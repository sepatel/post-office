-- A matching rule normally claims the email so lower-priority rules stop.
-- Opting in lets a rule act and still hand the email onward, e.g. a classifier
-- that labels the message and lets a later rule archive it.
ALTER TABLE rules ADD COLUMN continue_after_match INTEGER NOT NULL DEFAULT 0;
