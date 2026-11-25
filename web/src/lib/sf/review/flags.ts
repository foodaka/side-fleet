// What Sidefleet sees in a finished task's changes and pull request, as flags for the
// review desk. Every flag comes from the diff or the PR as GitHub reports it, never from
// what the agent says about its own work: that's what makes them worth trusting.

import type { DiffFile, DiffSet } from '../diff/parse';
import type { PullRequest } from '../types';

export interface Flag {
	/** Stable within a thread, so "Looks right" sticks across rounds. */
	id: string;
	/** `look`: worth a look before merging. `note`: worth knowing. */
	weight: 'look' | 'note';
	text: string;
	detail: string;
	/** How Sidefleet knows. */
	seen: string;
	/** A changed file to jump to, when the flag is about one. */
	file?: string;
}

const TEST = [/(^|\/)(tests?|__tests__|specs?|testdata|e2e)\//i, /[._-](test|spec)\.[a-z0-9]+$/i, /(^|\/)test_[^/]+\.py$/i, /_test\.(go|rs|py|rb|exs?)$/i, /Tests?\.swift$/];
const MANIFEST = /(^|\/)(Cargo\.toml|package\.json|pyproject\.toml|requirements[^/]*\.txt|go\.mod|Gemfile|composer\.json|Package\.swift|build\.gradle(\.kts)?|pom\.xml|mix\.exs)$/;
const LOCKFILE = /(^|\/)(Cargo\.lock|package-lock\.json|pnpm-lock\.yaml|yarn\.lock|bun\.lockb?|poetry\.lock|uv\.lock|go\.sum|Gemfile\.lock|composer\.lock|Package\.resolved|mix\.lock)$/;
const MIGRATION = /(^|\/)(migrations?|db\/migrate|alembic\/versions)\//i;
const CI = /(^|\/)(\.github\/workflows\/|\.gitlab-ci\.yml$|\.circleci\/|Dockerfile$|docker-compose[^/]*\.ya?ml$|\.buildkite\/)/;
const SECRETISH = /(^|\/)(\.env(\.[^/]*)?|[^/]*\.pem|[^/]*\.key|id_(rsa|ed25519)[^/]*|credentials[^/]*|secrets?\.[a-z]+)$/i;

export const isTest = (path: string) => TEST.some((re) => re.test(path));
const names = (fs: DiffFile[], max = 3) => fs.slice(0, max).map((f) => f.path).join(', ') + (fs.length > max ? ` and ${fs.length - max} more` : '');

/** Flags for a task's changes so far and its pull request, most important first. */
export function flagsFor(changes: DiffSet | null, pr: PullRequest | null | undefined): Flag[] {
	const out: Flag[] = [];
	const files = changes?.files ?? [];

	// An existing test that lost lines: it can be right, and it's also how a test gets bent to pass.
	const tests = files.filter((f) => isTest(f.path) && (f.status === 'deleted' || (f.status !== 'added' && f.deletions > 0)));
	if (tests.length)
		out.push({
			id: 'test-edit', weight: 'look', file: tests[0].path,
			text: tests.length === 1 ? 'An existing test changed' : `${tests.length} existing tests changed`,
			detail: `${names(tests)}. That can be right; it's also how a test gets bent to pass.`,
			seen: 'From the diff: lines removed from a test file that was already there.'
		});

	const manifests = files.filter((f) => MANIFEST.test(f.path));
	if (manifests.length)
		out.push({
			id: 'deps', weight: 'look', file: manifests[0].path,
			text: 'Dependencies may have changed',
			detail: `${names(manifests)} changed.`,
			seen: 'From the diff: a dependency manifest changed.'
		});
	else if (files.some((f) => LOCKFILE.test(f.path)))
		out.push({ id: 'lockfile', weight: 'note', text: 'A lockfile changed', detail: names(files.filter((f) => LOCKFILE.test(f.path))) + ', with no manifest change.', seen: 'From the diff.' });

	const secretish = files.filter((f) => SECRETISH.test(f.path));
	if (secretish.length)
		out.push({ id: 'secrets', weight: 'look', file: secretish[0].path, text: 'A file that often holds secrets changed', detail: `${names(secretish)}.`, seen: 'From the diff: the file name.' });

	const ci = files.filter((f) => CI.test(f.path));
	if (ci.length)
		out.push({ id: 'ci', weight: 'look', file: ci[0].path, text: 'Build or CI setup changed', detail: `${names(ci)}.`, seen: 'From the diff.' });

	const migrations = files.filter((f) => MIGRATION.test(f.path) && f.status !== 'deleted');
	if (migrations.length)
		out.push({ id: 'migration', weight: 'look', file: migrations[0].path, text: 'A database migration', detail: `${names(migrations)}. It runs against your database once merged and deployed.`, seen: 'From the diff.' });

	if (pr && pr.state === 'open') {
		if (pr.conflicts) out.push({ id: 'conflicts', weight: 'look', text: `#${pr.number} has conflicts`, detail: `GitHub can't merge it into ${pr.base ?? 'its base'} cleanly.`, seen: 'From GitHub, when the machine last looked.' });
		if (pr.checks?.failed)
			out.push({
				id: 'checks-failed', weight: 'look',
				text: pr.checks.failed === 1 ? 'A check is failing' : `${pr.checks.failed} checks are failing`,
				detail: pr.checks.failing?.map((c) => c.name).join(', ') || 'On the latest commit.',
				seen: 'From GitHub, when the machine last looked.'
			});
		if (pr.review === 'changes_requested') out.push({ id: 'changes-requested', weight: 'look', text: 'A reviewer asked for changes', detail: `On #${pr.number}.`, seen: 'From GitHub.' });
		if (pr.checks?.pending) out.push({ id: 'checks-pending', weight: 'note', text: 'Checks still running', detail: `${pr.checks.pending} of ${pr.checks.passed + pr.checks.failed + pr.checks.pending}.`, seen: 'From GitHub, when the machine last looked.' });
	}

	const hidden = files.filter((f) => f.omitted === 'large' || f.omitted === 'budget');
	if (hidden.length || (changes && changes.file_count > files.length))
		out.push({ id: 'hidden', weight: 'note', text: 'Some changes are too big to show here', detail: hidden.length ? names(hidden) + '.' : `${changes!.file_count - files.length} files.`, seen: 'From the diff.' });

	return out;
}

/** Plain facts about the change, for the line under the flags. */
export function factsFor(changes: DiffSet | null, pr: PullRequest | null | undefined): string[] {
	const out: string[] = [];
	if (changes) out.push(`${changes.file_count} ${changes.file_count === 1 ? 'file' : 'files'}, +${changes.additions} −${changes.deletions}`);
	const added = changes?.files.filter((f) => isTest(f.path) && f.status === 'added').length ?? 0;
	if (added) out.push(`${added} new test ${added === 1 ? 'file' : 'files'}`);
	if (pr?.checks && !pr.checks.failed && !pr.checks.pending) out.push(`${pr.checks.passed} ${pr.checks.passed === 1 ? 'check passes' : 'checks pass'} on #${pr.number}`);
	if (pr?.review === 'approved') out.push(`#${pr.number} is approved`);
	if (pr && !pr.checks && pr.state === 'open') out.push(`#${pr.number} has no checks`);
	return out;
}
