import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test, expect } from '@playwright/test';

// B-28 smoke: notes create + list through the graph routes after an auth
// session. The notes path is read from ../module.toml so the spec follows
// whatever project name `keel new` rendered (no hardcoded placeholder).

const manifest = readFileSync(resolve(__dirname, '..', 'module.toml'), 'utf8');
const notesPath =
  /path = "(\/[^"]*\/notes)"/.exec(manifest)?.[1] ?? '/api/v1/__PROJECT_NAME__/notes';

const uniqueEmail = () =>
  `e2e-notes-${Date.now()}-${Math.floor(Math.random() * 1e6)}@starter.test`;

const password = 'correct-horse-99';

test('create then list round-trips a note under a session', async ({
  request,
}) => {
  const reg = await request.post('/auth/register', {
    data: { email: uniqueEmail(), password, tenant: 'dev' },
  });
  expect(reg.ok(), await reg.text()).toBeTruthy();

  const created = await request.post(notesPath, {
    data: { body: 'playwright smoke note' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const createdBody = await created.json();
  expect(createdBody.ok).toBe(true);
  expect(createdBody.data.id).toBeTruthy();

  const listed = await request.get(notesPath);
  expect(listed.ok()).toBeTruthy();
  const listBody = await listed.json();
  expect(listBody.ok).toBe(true);
  const notes = listBody.data.notes;
  expect(Array.isArray(notes)).toBeTruthy();
  expect(
    notes.some((note: { body: string }) => note.body === 'playwright smoke note'),
  ).toBeTruthy();
});

test('notes require a session (anonymous create is refused)', async ({
  request,
}) => {
  const res = await request.post(notesPath, { data: { body: 'no session' } });
  expect(res.status()).toBeGreaterThanOrEqual(400);
  const body = await res.json();
  expect(body.ok).toBe(false);
  expect(body.error.code).toBeTruthy();
});
