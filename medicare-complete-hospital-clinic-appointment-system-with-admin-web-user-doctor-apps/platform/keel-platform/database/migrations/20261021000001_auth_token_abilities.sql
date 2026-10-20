-- LF-056: personal access token ABILITIES.
--
-- A PAT is a narrower credential than its principal: it may exercise what its
-- abilities allow and nothing more, so a leaked token cannot act as the whole
-- account. The narrowing must live in the DURABLE row (it is part of the
-- credential, not of the request), so the column carries it.
--
-- Expand: add the column with an empty default - every existing row keeps
-- working (an old row simply grants no extra ability).
ALTER TABLE auth_tokens
    ADD COLUMN IF NOT EXISTS abilities JSONB NOT NULL DEFAULT '[]'::jsonb;
