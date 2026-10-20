// Fetch wrapper for the developer API. The key lives in localStorage and only
// ever travels as an Authorization header to this same origin.

const KEY = 'sidefleet_dev_key';

export function getKey(): string {
	try {
		return localStorage.getItem(KEY) ?? '';
	} catch {
		return '';
	}
}

export function setKey(k: string) {
	try {
		localStorage.setItem(KEY, k);
	} catch {
		/* private mode: key lasts for this tab only */
	}
	memoryKey = k;
}

let memoryKey = '';
export const currentKey = () => memoryKey || getKey();

export class Unauthorized extends Error {
	constructor() {
		super('unauthorised');
	}
}

export async function api<T = any>(path: string, init: RequestInit = {}): Promise<T> {
	const headers = new Headers(init.headers);
	headers.set('Authorization', 'Bearer ' + currentKey());
	if (init.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
	const r = await fetch(path, { ...init, headers });
	if (r.status === 401) throw new Unauthorized();
	if (!r.ok) {
		let msg = r.statusText;
		try {
			msg = (await r.json()).error ?? msg;
		} catch {
			/* not JSON */
		}
		throw new Error(msg);
	}
	return (r.status === 204 || r.status === 202 && r.headers.get('content-length') === '0' ? null : r.json()) as T;
}

/** Check a key without storing it. */
export async function keyWorks(k: string): Promise<boolean> {
	const r = await fetch('/v1/workers', { headers: { Authorization: 'Bearer ' + k } });
	return r.status !== 401;
}
