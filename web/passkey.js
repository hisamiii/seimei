// WebAuthn glue: the server sends options as JSON with base64url fields, the browser wants ArrayBuffers.

const b64 = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const unb64 = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4)), (c) => c.charCodeAt(0));

export const supported = () => !!window.PublicKeyCredential;

export async function api(path, body = {}) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const data = await r.json().catch(() => ({}));
  return { status: r.status, ok: r.ok, data };
}

function toJSON(c) {
  const r = c.response;
  const out = {
    id: c.id, rawId: b64(c.rawId), type: c.type,
    authenticatorAttachment: c.authenticatorAttachment ?? null,
    clientExtensionResults: c.getClientExtensionResults?.() ?? {},
    response: { clientDataJSON: b64(r.clientDataJSON) },
  };
  if (r.attestationObject) {
    out.response.attestationObject = b64(r.attestationObject);
    out.response.transports = r.getTransports?.() ?? [];
  } else {
    out.response.authenticatorData = b64(r.authenticatorData);
    out.response.signature = b64(r.signature);
    out.response.userHandle = r.userHandle ? b64(r.userHandle) : null;
  }
  return out;
}

// Returns { ok } or { ok: false, status, cancelled }.
export async function login() {
  const o = await api('api/auth/passkey/options');
  if (!o.ok) return { ok: false, status: o.status };
  const opts = o.data;
  opts.challenge = unb64(opts.challenge);
  opts.allowCredentials = (opts.allowCredentials || []).map((c) => ({ ...c, id: unb64(c.id) }));
  let cred;
  try {
    cred = await navigator.credentials.get({ publicKey: opts });
  } catch {
    return { ok: false, cancelled: true };
  }
  const r = await api('api/auth/passkey/login', toJSON(cred));
  return { ok: r.ok, status: r.status };
}

export async function register(name) {
  const o = await api('api/auth/passkey/register/options');
  if (!o.ok) return { ok: false, status: o.status };
  const opts = o.data;
  opts.challenge = unb64(opts.challenge);
  opts.user.id = unb64(opts.user.id);
  opts.excludeCredentials = (opts.excludeCredentials || []).map((c) => ({ ...c, id: unb64(c.id) }));
  let cred;
  try {
    cred = await navigator.credentials.create({ publicKey: opts });
  } catch (e) {
    return { ok: false, cancelled: e.name === 'NotAllowedError', exists: e.name === 'InvalidStateError' };
  }
  const r = await api('api/auth/passkey/register', { credential: toJSON(cred), name });
  return { ok: r.ok, status: r.status };
}

export function deviceName() {
  const ua = navigator.userAgent;
  const kind = /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iPhone' : /Linux/.test(ua) ? 'Linux' : /Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : 'Browser';
  return `${kind}, ${new Date().toLocaleDateString()}`;
}
