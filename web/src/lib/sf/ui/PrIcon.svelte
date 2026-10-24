<script lang="ts">
	// A pull request's state as GitHub draws it: open, merged or closed.
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import GitMerge from '@lucide/svelte/icons/git-merge';
	import GitPullRequestClosed from '@lucide/svelte/icons/git-pull-request-closed';
	import type { PullRequest } from '../types';

	let { state, class: className = 'size-3' }: { state: PullRequest['state']; class?: string } = $props();
</script>

{#if state === 'merged'}
	<GitMerge class="{className} shrink-0 text-merged-ink" />
{:else if state === 'closed'}
	<GitPullRequestClosed class="{className} shrink-0 text-err-ink" />
{:else}
	<GitPullRequest class="{className} shrink-0 text-ok-ink" />
{/if}
