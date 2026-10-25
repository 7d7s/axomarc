import { test, expect } from '@playwright/test';

// B-28 smoke: the platform auth surface (register + login) against a running
// `kelson serve`. Unique email per run so repeats never collide with an
// existing principal; cookies flow through the per-test API request context.

const uniqueEmail = (tag: string) =>
  `e2e-${tag}-${Date.now()}-${Math.floor(Math.random() * 1e6)}@starter.test`;

const password = 'correct-horse-99';

// MNC-005: `/auth/register` answers ONE uniform 202 — it mints no session
// and never reveals whether the address was taken. The session is what the
// explicit sign-in that follows issues.
test('register answers 202 with no session, and sign-in issues it', async ({
  request,
}) => {
  const email = uniqueEmail('register');
  const reg = await request.post('/auth/register', {
    data: { email, password, tenant: 'dev' },
  });
  expect(reg.status(), await reg.text()).toBe(202);
  const regBody = await reg.json();
  expect(regBody.ok).toBe(true);
  expect(regBody.data.sent).toBe(true);

  // Registration alone leaves the caller anonymous.
  const anon = await request.get('/auth/me');
  expect(anon.status()).toBe(401);

  // The explicit sign-in is what issues the session.
  const login = await request.post('/auth/login', {
    data: { identifier: email, password, tenant: 'dev' },
  });
  expect(login.ok(), await login.text()).toBeTruthy();

  const me = await request.get('/auth/me');
  expect(me.ok()).toBeTruthy();
  const meBody = await me.json();
  expect(meBody.ok).toBe(true);
  expect(meBody.data.principal).toBeTruthy();
});

test('register answers 202 and a bad password still hits the same 401', async ({
  request,
}) => {
  const email = uniqueEmail('badlogin');
  const reg = await request.post('/auth/register', {
    data: { email, password, tenant: 'dev' },
  });
  // MNC-005: uniform 202 — the register step says nothing about the address.
  expect(reg.status(), await reg.text()).toBe(202);

  const res = await request.post('/auth/login', {
    data: { identifier: email, password: 'wrong-password', tenant: 'dev' },
  });
  expect(res.status()).toBe(401);
  const body = await res.json();
  expect(body.ok).toBe(false);
  expect(body.error.code).toBeTruthy();
});

test('login with the right password resolves the same principal', async ({
  request,
}) => {
  const email = uniqueEmail('login');
  const reg = await request.post('/auth/register', {
    data: { email, password, tenant: 'dev' },
  });
  expect(reg.status(), await reg.text()).toBe(202);

  const login = await request.post('/auth/login', {
    data: { identifier: email, password, tenant: 'dev' },
  });
  expect(login.ok(), await login.text()).toBeTruthy();
  const loginBody = await login.json();
  expect(loginBody.ok).toBe(true);

  const me = await request.get('/auth/me');
  const meBody = await me.json();
  expect(meBody.ok).toBe(true);
  expect(meBody.data.principal).toBeTruthy();
});
