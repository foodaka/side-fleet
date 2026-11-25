<script lang="ts">
	// A pull request's state as GitHub draws it: open, draft, merged or closed.
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import GitMerge from '@lucide/svelte/icons/git-merge';
	import GitPullRequestClosed from '@lucide/svelte/icons/git-pull-request-closed';
	import GitPullRequestDraft from '@lucide/svelte/icons/git-pull-request-draft';
	import type { PullRequest } from '../types';

	let { state, draft = false, class: className = 'size-3' }: { state: PullRequest['state']; draft?: boolean; class?: string } = $props();
</script>

{#if state === 'merged'}
	<GitMerge class="{className} shrink-0 text-merged-ink" />
{:else if state === 'closed'}
	<GitPullRequestClosed class="{className} shrink-0 text-err-ink" />
{:else if draft}
	<GitPullRequestDraft class="{className} shrink-0 text-muted-foreground" />
{:else}
	<GitPullRequest class="{className} shrink-0 text-ok-ink" />
{/if}
