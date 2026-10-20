import type { RunSummary, Worker } from './types';

export type FleetEvent = { kind: 'beat' | 'dispatch' | 'done'; worker: string };

/**
 * Turns successive snapshots of the fleet into events the Machines page can
 * animate (a heartbeat arrived, a task started or finished on a machine) and a
 * short CPU history per machine, collected while the page is open.
 */
class Telemetry {
	/** Last 40 CPU readings per worker id (about ten minutes at one reading per heartbeat). */
	cpu = $state<Record<string, number[]>>({});
	#beats = new Map<string, string>();
	#runs = new Map<string, string>();
	#listeners = new Set<(e: FleetEvent) => void>();
	#primed = false;

	on(fn: (e: FleetEvent) => void) {
		this.#listeners.add(fn);
		return () => this.#listeners.delete(fn);
	}

	#emit(e: FleetEvent) {
		this.#listeners.forEach((fn) => fn(e));
	}

	ingest(workers: Worker[], runs: RunSummary[]) {
		for (const w of workers) {
			const prev = this.#beats.get(w.id);
			if (prev !== w.last_heartbeat_at) {
				this.#beats.set(w.id, w.last_heartbeat_at);
				const sys = w.capabilities.system;
				if (sys) {
					const h = this.cpu[w.id] ?? [];
					this.cpu[w.id] = [...h, sys.cpu_percent].slice(-40);
				}
				if (this.#primed && prev && w.online) this.#emit({ kind: 'beat', worker: w.id });
			}
		}
		for (const r of runs) {
			const prev = this.#runs.get(r.id);
			const now = r.thread_status;
			this.#runs.set(r.id, now);
			if (!this.#primed || prev === now || !r.worker_id) continue;
			if (now === 'running') this.#emit({ kind: 'dispatch', worker: r.worker_id });
			else if (prev === 'running') this.#emit({ kind: 'done', worker: r.worker_id });
		}
		this.#primed = true;
	}
}

export const telemetry = new Telemetry();
