<script lang="ts">
	import { onMount } from 'svelte';
	import { Message, MessageContent } from '#lib/components/ai-elements/message';
	import { getStickToBottomContext } from '#lib/components/ai-elements/conversation/stick-to-bottom-context.svelte.js';
	import type { Outgoing } from '../thread.svelte';

	let { msg, onretry, ondiscard }: { msg: Outgoing; onretry: () => void; ondiscard: () => void } = $props();
	const scroll = getStickToBottomContext();
	// You just sent it: bring it into view, even if you had scrolled up.
	onMount(() => scroll.scrollToBottom());
</script>

<!-- Looks like the turn that replaces it (same bubble), so nothing moves when the server confirms. -->
<div class="flex flex-col gap-4">
	<Message from="user">
		<MessageContent class="max-w-[85%] rounded-2xl rounded-br-md bg-primary px-4 py-2.5 text-[14px] leading-relaxed whitespace-pre-wrap text-primary-foreground transition-opacity group-[.is-user]:bg-primary group-[.is-user]:text-primary-foreground {msg.status === 'failed' ? 'opacity-60' : 'opacity-75'}">{msg.text}</MessageContent>
		{#if msg.status === 'failed'}
			<span class="flex flex-wrap items-center justify-end gap-x-2 gap-y-1 self-end text-[11px] text-err-ink" role="alert">
				<span>Couldn't send{msg.error ? `: ${msg.error}` : ''}</span>
				<button type="button" class="rounded-full bg-primary px-2.5 py-1 font-medium whitespace-nowrap text-primary-foreground hover:opacity-90" onclick={onretry}>Retry</button>
				<button type="button" class="rounded-full px-2 py-1 whitespace-nowrap text-muted-foreground hover:bg-accent hover:text-foreground" onclick={ondiscard}>Discard</button>
			</span>
		{/if}
	</Message>
</div>
