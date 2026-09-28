-- B-23 email verification: record when a principal proved ownership of the
-- contact email. NULL = unverified. Additive column only (forward-only
-- migration discipline, G08); existing rows stay unverified until proven.
ALTER TABLE principals ADD COLUMN IF NOT EXISTS email_verified_at timestamptz;
