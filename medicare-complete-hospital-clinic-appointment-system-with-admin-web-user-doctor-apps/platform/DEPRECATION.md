# Deprecation Policy

Nothing public disappears quietly.

## Flow
1. Mark: document the deprecation in CHANGELOG + code doc (`deprecated` note).
2. Bridge: provide the replacement path; keep both working for at least one
   release (two for wire contracts).
3. Remove: only in a release explicitly announced as breaking.

## Special cases
- **Error codes**: never removed; may be marked historical.
- **Operations**: replaced by a new operation id; routes may alias during the
  bridge window.
- **DB columns**: two-phase (stop writing → backfill → drop in a later release).