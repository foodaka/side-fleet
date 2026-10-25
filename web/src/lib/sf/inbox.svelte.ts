import { api, authHeaders } from './api';
import type { InboxItem } from './types';

// The "waiting on you" inbox: tasks that finished, failed, paused on a usage limit
// or lost their machine, until you open or dismiss them. The control plane keeps
// what you have seen, so it is the same on every device; this follows its stream
// (`/v1/inbox/events`) and handles the opt-in web push ("Notify me").

type PushState = 'unsupported' | 'insecure' | 'unavailable' | 'denied' | 'off' | 'on';

const SW = '/service-worker.js';

function keyBytes(b64url: string): Uint8Array<ArrayBuffer> {
	const b64 = (b64url + '='.repeat((4 - (b64url.length % 4)) % 4)).replace(/-/g, '+').replace(/_/g, '/');
	const raw = atob(b64);
	const out = new Uint8Array(new ArrayBuffer(raw.length));
	for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
	return out;
}

class Inbox {
	items = $state<InboxItem[]>([]);
	loaded = $state(false);
	/** Whether this tab is in front: an open task only counts as seen while you can see it. */
	visible = $state(typeof document === 'undefined' || document.visibilityState === 'visible');
	push = $state<PushState>('off');
	pushBusy = $state(false);
	pushError = $state('');

	/** Task id or run id -> its inbox item. */
	byId = $derived(new Map(this.items.flatMap((i) => [[i.id, i] as const, [i.run_id, i] as const])));

	#abort: AbortController | null = null;
	#key = '';
	#wired = false;

	/** Follow the inbox of whoever is signed in, in the current org. Call again after switching org. */
	connect() {
		this.#abort?.abort();
		const abort = (this.#abort = new AbortController());
		this.items = [];
		this.loaded = false;
		void this.#follow(abort.signal);
		if (!this.#wired) {
			this.#wired = true;
			document.addEventListener('visibilitychange', () => (this.visible = document.visibilityState === 'visible'));
			// A notification was clicked while the page was open: go to its task.
			navigator.serviceWorker?.addEventListener('message', (e) => {
				if (e.data?.type === 'open' && typeof e.data.url === 'string') location.hash = new URL(e.data.url, location.href).hash;
			});
			void this.#initPush();
		}
	}

	disconnect() {
		this.#abort?.abort();
		this.#abort = null;
		this.items = [];
	}

	async #follow(signal: AbortSignal) {
		while (!signal.aborted) {
			try {
				const r = await fetch('/v1/inbox/events', { headers: authHeaders(), credentials: 'same-origin', signal });
				if (r.status === 401) return; // signed out; the page asks for a key and reconnects
				if (!r.ok || !r.body) throw new Error('inbox stream ' + r.status);
				const reader = r.body.getReader();
				const dec = new TextDecoder();
				let buf = '';
				for (;;) {
					const { value, done } = await reader.read();
					if (done) break;
					buf += dec.decode(value, { stream: true });
					let i: number;
					while ((i = buf.indexOf('\n\n')) >= 0) {
						const frame = buf.slice(0, i);
						buf = buf.slice(i + 2);
						if (!frame.split('\n').includes('event: inbox')) continue;
						const data = frame.split('\n').filter((l) => l.startsWith('data:')).map((l) => l.slice(5).trim()).join('');
						try {
							this.items = (JSON.parse(data) as { items: InboxItem[] }).items;
							this.loaded = true;
						} catch {
							/* ignore a malformed frame */
						}
					}
				}
			} catch {
				if (signal.aborted) return;
			}
			await new Promise((res) => setTimeout(res, 3000));
		}
	}

	/** Seen: out of your inbox until it needs you again. Any run id of the task works. */
	async dismiss(id: string) {
		const item = this.byId.get(id);
		if (item) this.items = this.items.filter((i) => i !== item);
		try {
			await api(`/v1/inbox/${item?.id ?? id}/dismiss`, { method: 'POST' });
		} catch {
			/* the stream puts it back if it did not stick */
		}
	}

	async clearAll() {
		this.items = [];
		try {
			await api('/v1/inbox/dismiss-all', { method: 'POST' });
		} catch {
			/* the stream puts them back */
		}
	}

	// ---- web push ----

	async #initPush() {
		if (!('serviceWorker' in navigator) || !('PushManager' in window) || !('Notification' in window)) return void (this.push = 'unsupported');
		// Browsers only allow push on HTTPS pages and on localhost.
		if (!isSecureContext) return void (this.push = 'insecure');
		try {
			const info = await api<{ available: boolean; public_key: string | null }>('/v1/push');
			if (!info.available || !info.public_key) return void (this.push = 'unavailable');
			this.#key = info.public_key;
		} catch {
			return void (this.push = 'unavailable');
		}
		if (Notification.permission === 'denied') return void (this.push = 'denied');
		const reg = await navigator.serviceWorker.getRegistration(SW).catch(() => undefined);
		const sub = await reg?.pushManager.getSubscription().catch(() => null);
		if (sub && Notification.permission === 'granted' && this.#sameKey(sub)) {
			this.push = 'on';
			// Keep the control plane's copy current (and tied to whoever is signed in now).
			void api('/v1/push/subscriptions', { method: 'POST', body: JSON.stringify(sub.toJSON()) }).catch(() => {});
		} else {
			this.push = 'off';
		}
	}

	#sameKey(sub: PushSubscription): boolean {
		const k = sub.options.applicationServerKey;
		if (!k) return false;
		const a = new Uint8Array(k), b = keyBytes(this.#key);
		return a.length === b.length && a.every((x, i) => x === b[i]);
	}

	async setPush(on: boolean) {
		if (this.pushBusy) return;
		this.pushBusy = true;
		this.pushError = '';
		try {
			if (on) await this.#enablePush();
			else await this.#disablePush();
		} catch (e) {
			this.pushError = (e as Error).message || 'Could not change notifications';
		} finally {
			this.pushBusy = false;
		}
	}

	async #enablePush() {
		const perm = await Notification.requestPermission();
		if (perm !== 'granted') {
			this.push = perm === 'denied' ? 'denied' : 'off';
			return;
		}
		const reg = await navigator.serviceWorker.register(SW, { scope: '/' });
		await navigator.serviceWorker.ready;
		// A subscription made with an older key cannot be reused.
		const old = await reg.pushManager.getSubscription();
		if (old && !this.#sameKey(old)) await old.unsubscribe();
		const sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(this.#key) });
		await api('/v1/push/subscriptions', { method: 'POST', body: JSON.stringify(sub.toJSON()) });
		this.push = 'on';
	}

	async #disablePush() {
		const reg = await navigator.serviceWorker.getRegistration(SW);
		const sub = await reg?.pushManager.getSubscription();
		if (sub) {
			await api('/v1/push/subscriptions', { method: 'DELETE', body: JSON.stringify({ endpoint: sub.endpoint }) }).catch(() => {});
			await sub.unsubscribe();
		}
		this.push = 'off';
	}
}

export const inbox = new Inbox();

/** Colour of a reason's marker (the status palette). */
export function reasonTone(reason: string): string {
	switch (reason) {
		case 'finished':
			return 'bg-ok';
		case 'usage_limit':
			return 'bg-queue';
		case 'approval_needed':
			return 'bg-run';
		default:
			return 'bg-err';
	}
}
