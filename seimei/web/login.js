import { api, login, supported } from './passkey.js';

const $ = (id) => document.getElementById(id);

// Logged out: no conversations stay in this browser (util.js keeps them while logged in).
try {
  for (const k of Object.keys(localStorage)) if (k.startsWith('seimei.keep.')) localStorage.removeItem(k);
} catch {}
const err = $('err');
const say = (text) => { err.textContent = text; };

if (!supported()) {
  $('passkey').hidden = true;
  $('or').hidden = true;
}

$('passkey').addEventListener('click', async () => {
  say('');
  $('passkey').disabled = true;
  const r = await login().catch(() => ({ ok: false }));
  $('passkey').disabled = false;
  if (r.ok) return location.reload();
  if (r.cancelled) return;
  say(r.status === 409 ? 'No fingerprint set up yet. Log in with the password, then add it from the menu.'
    : 'That did not work. Try again or use the password.');
});

$('pwform').addEventListener('submit', async (e) => {
  e.preventDefault();
  say('');
  $('pwbtn').disabled = true;
  const r = await api('api/auth/password', { password: $('pw').value }).catch(() => ({ ok: false, data: {} }));
  $('pwbtn').disabled = false;
  if (r.ok) return location.reload();
  $('pw').select();
  say(r.data.wait ? `Too many tries. Wait ${r.data.wait} s.` : r.status === 401 ? 'Wrong password.' : 'Could not reach the laptop.');
});
