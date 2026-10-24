import { authHeaders } from './api';
import type { RunEvent } from './types';

/**
 * Follow a run's event stream: replay from the start, then live. Uses fetch
 * (EventSource cannot send the Authorization or org header) and resumes with
 * Last-Event-ID after a dropped connection. Resolves when the run ends.
 */
export async function followRun(
	runId: string,
	onEvent: (ev: RunEvent) => void,
	signal: AbortSignal,
	onReconnect?: () => void
): Promise<void> {
	let lastSeq = -1;
	while (!signal.aborted) {
		try {
			const r = await fetch(`/v1/runs/${runId}/events`, {
				headers: authHeaders(new Headers({ 'Last-Event-ID': String(lastSeq) })),
				credentials: 'same-origin',
				signal
			});
			if (!r.ok || !r.body) throw new Error('stream ' + r.status);
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
					let name = 'message', data = '', id: string | null = null;
					for (const line of frame.split('\n')) {
						if (line.startsWith('event:')) name = line.slice(6).trim();
						else if (line.startsWith('data:')) data += line.slice(5).trim();
						else if (line.startsWith('id:')) id = line.slice(3).trim();
					}
					if (name === 'end') return;
					if (!data || name === 'error') continue;
					try {
						const ev = JSON.parse(data) as RunEvent;
						if (id != null) lastSeq = Number(id);
						onEvent(ev);
					} catch {
						/* ignore a malformed frame */
					}
				}
			}
		} catch {
			if (signal.aborted) return;
			onReconnect?.();
		}
		await new Promise((res) => setTimeout(res, 1500));
	}
}
