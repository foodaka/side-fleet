// Fetch wrapper for the developer API. Two ways in: a developer key (self-hosted;
// kept in localStorage, sent only as an Authorization header to this same origin)
// or a GitHub sign-in session (an httpOnly cookie the browser sends by itself).
// With a session, `X-Sidefleet-Org` says which of the user's orgs to act in.

const KEY = 'sidefleet_dev_key';
const ORG = 'sidefleet_org';

function load(k: string): string {
	try {
		return localStorage.getItem(k) ?? '';
	} catch {
		return '';
	}
}

function save(k: string, v: string) {
	try {
		if (v) localStorage.setItem(k, v);
		else localStorage.removeItem(k);
	} catch {
		/* private mode: lasts for this tab only */
	}
}

export function getKey(): string {
	return load(KEY);
}

export function setKey(k: string) {
	save(KEY, k);
	memoryKey = k;
}

let memoryKey = '';
export const currentKey = () => memoryKey || getKey();

let memoryOrg = '';
export const currentOrg = () => memoryOrg || load(ORG);
export function setOrg(id: string) {
	save(ORG, id);
	memoryOrg = id;
}

/** Headers every API call carries: the developer key if there is one, else the chosen org (unless the call names one). */
export function authHeaders(h: Headers = new Headers()): Headers {
	const k = currentKey();
	if (k) h.set('Authorization', 'Bearer ' + k);
	const o = currentOrg();
	if (o && !k && !h.has('X-Sidefleet-Org')) h.set('X-Sidefleet-Org', o);
	return h;
}

export class Unauthorized extends Error {
	constructor() {
		super('unauthorised');
	}
}

/** An error the control plane explained, with its HTTP status (402 for plan limits, 403 for roles). */
export class ApiError extends Error {
	constructor(
		message: string,
		public status: number
	) {
		super(message);
	}
}

export async function api<T = any>(path: string, init: RequestInit = {}): Promise<T> {
	const headers = authHeaders(new Headers(init.headers));
	if (init.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
	const r = await fetch(path, { ...init, headers, credentials: 'same-origin' });
	if (r.status === 401) throw new Unauthorized();
	if (!r.ok) {
		let msg = r.statusText;
		try {
			msg = (await r.json()).error ?? msg;
		} catch {
			/* not JSON */
		}
		throw new ApiError(msg, r.status);
	}
	return (r.status === 204 || r.status === 202 && r.headers.get('content-length') === '0' ? null : r.json()) as T;
}

/** Check a key without storing it. */
export async function keyWorks(k: string): Promise<boolean> {
	const r = await fetch('/v1/workers', { headers: { Authorization: 'Bearer ' + k } });
	return r.status !== 401;
}
