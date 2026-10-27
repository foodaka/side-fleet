// An invite opened before signing in: remembered across the GitHub round trip, which drops the hash.
export const JOIN_KEY = 'sidefleet_join';

export function pendingJoin(): string {
	try {
		return localStorage.getItem(JOIN_KEY) ?? '';
	} catch {
		return '';
	}
}
