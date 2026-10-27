// Rooms: a shared goal split into tasks. Each task runs on its owner's machine; finished work is
// merged into the room's branch, and new tasks in the room start from it.
import { fleet } from './state.svelte';
import type { Room } from './types';

/** `github.com/owner/repo` for the room's repo: its identity, or one a machine reports for that folder. */
export function githubRepo(repo: string): string | null {
	const gh = (k: string) => (/^github\.com\/[^/]+\/[^/]+$/.test(k) ? k : null);
	if (gh(repo)) return repo;
	for (const w of fleet.workers) for (const r of w.capabilities.repos ?? []) if (r.name === repo && r.id && gh(r.id)) return r.id;
	return null;
}

/** GitHub's compare page for the room branch, where a pull request is one click away. */
export function compareUrl(room: Room): string | null {
	const gh = githubRepo(room.repo);
	return gh ? `https://${gh}/compare/${room.branch.split('/').map(encodeURIComponent).join('/')}?expand=1` : null;
}

/** The reply that asks a task's agent to take in the room's work and resolve the conflicts. */
export const fixConflictsPrompt = (branch: string, files: string[]) =>
	`Adding this task to the room failed: its changes conflict with work already on the room branch` +
	(files.length ? ` (${files.join(', ')})` : '') +
	`. Fetch origin and merge origin/${branch} into this branch, resolve the conflicts so both behaviours are kept, ` +
	`make sure it builds and the tests pass, and commit. Then I'll add it to the room again.`;
