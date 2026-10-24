<script lang="ts">
	// The feedback dialog (from the sidebar). It posts to the Sidefleet site, which forwards it to
	// the maintainer's Telegram, so it works from any self-hosted control plane.
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { Textarea } from '#lib/components/ui/textarea';
	import { toast } from 'svelte-sonner';
	import { fleet } from '../state.svelte';

	const ENDPOINT = 'https://www.sidefleet.xyz/api/feedback';

	let message = $state('');
	let email = $state('');
	let company = $state('');
	let busy = $state(false);

	async function send(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const r = await fetch(ENDPOINT, {
				method: 'POST',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({ message: message.trim(), email: email.trim(), build: fleet.setup?.build ?? '', company })
			});
			const data = await r.json().catch(() => ({}));
			if (!r.ok) throw new Error(data.error || 'Could not send your feedback');
			fleet.feedbackOpen = false;
			message = '';
			toast.success('Thanks, your feedback was sent');
		} catch (err) {
			toast.error(err instanceof TypeError ? 'Could not reach sidefleet.xyz' : err instanceof Error ? err.message : 'Could not send your feedback');
		} finally {
			busy = false;
		}
	}
</script>

<Dialog.Root bind:open={fleet.feedbackOpen}>
	<Dialog.Content class="sm:max-w-[28rem]">
		<Dialog.Header>
			<Dialog.Title>Send feedback</Dialog.Title>
			<Dialog.Description>A bug, an idea, something that got in the way. It goes straight to the person who makes Sidefleet.</Dialog.Description>
		</Dialog.Header>
		<form onsubmit={send} class="flex flex-col gap-3">
			<Textarea bind:value={message} placeholder="What's on your mind?" maxlength={4000} rows={5} class="min-h-28 text-[13px]" aria-label="Feedback" />
			<Input bind:value={email} type="email" placeholder="Email, if you'd like a reply (optional)" maxlength={254} class="h-9 text-[13px]" aria-label="Email" />
			<input bind:value={company} name="company" tabindex="-1" autocomplete="off" aria-hidden="true" class="hidden" />
			<Button type="submit" class="h-9 rounded-full" disabled={busy || !message.trim()}>{busy ? 'Sending…' : 'Send feedback'}</Button>
		</form>
	</Dialog.Content>
</Dialog.Root>
