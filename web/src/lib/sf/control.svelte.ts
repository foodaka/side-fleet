// What the sidebar shows for the control plane: how many approvals wait and how many agents
// there are. Polled while something on the page uses it; pages call `refresh()` after a change.

import { listAgents, listApprovals } from './control';
import { fleet } from './state.svelte';

const EVERY = 15_000;

class ControlCounts {
	pending = $state(0);
	agents = $state<number | null>(null);
	/** Whether the control plane answers these routes at all (an older one doesn't). */
	available = $state(true);
	#users = 0;
	#timer: ReturnType<typeof setInterval> | null = null;
	#org = '';

	/** Start polling; returns the function that stops it again. */
	use(): () => void {
		this.#users++;
		if (!this.#timer) {
			void this.refresh();
			this.#timer = setInterval(() => void this.refresh(), EVERY);
		}
		return () => {
			this.#users--;
			if (this.#users <= 0 && this.#timer) {
				clearInterval(this.#timer);
				this.#timer = null;
			}
		};
	}

	async refresh() {
		if (!fleet.loaded || fleet.needsSignIn) return;
		const org = fleet.orgId;
		if (org !== this.#org) {
			this.#org = org;
			this.pending = 0;
			this.agents = null;
		}
		try {
			const [a, agents] = await Promise.all([listApprovals('pending'), listAgents()]);
			if (org !== fleet.orgId) return;
			this.pending = a.counts?.pending ?? a.approvals.length;
			this.agents = agents.length;
			this.available = true;
		} catch (e) {
			if ((e as { status?: number }).status === 404) this.available = false;
		}
	}
}

export const control = new ControlCounts();
