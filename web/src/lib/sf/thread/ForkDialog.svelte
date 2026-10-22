<script lang="ts">
	// Fork a thread at one message into a new thread, with any agent and model.
	import { goto } from '$app/navigation';
	import { toast } from 'svelte-sonner';
	import * as Dialog from '#lib/components/ui/dialog';
	import * as Select from '#lib/components/ui/select';
	import { Button } from '#lib/components/ui/button';
	import GitFork from '@lucide/svelte/icons/git-fork';
	import { api } from '../api';
	import { fleet } from '../state.svelte';
	import { PERMISSIONS, agentName, defaultModel, hasAgent, machineModels, modelValid, permHint, repoKey, repoLabel } from '../format';
	import type { AgentId, Run } from '../types';
	import AgentModelPicker from '../ui/AgentModelPicker.svelte';

	let { open = $bindable(false), run, hasFiles }: { open: boolean; run: Run; /** The message has a snapshot: the fork starts from its files, on its machine. */ hasFiles: boolean } = $props();

	let agent = $state<AgentId>('claude');
	let model = $state('');
	let perm = $state('acceptEdits');
	let task = $state('');
	let busy = $state(false);

	// Start from the thread's own agent and model each time it opens.
	$effect(() => {
		if (!open) return;
		agent = run.agent;
		model = run.model;
		perm = PERMISSIONS.some((p) => p.id === run.permission_mode) ? run.permission_mode! : 'acceptEdits';
		task = '';
	});

	/** Where the fork can run: the machine holding the files, or any online machine with the repo. */
	const machines = $derived(
		hasFiles
			? fleet.workers.filter((w) => w.id === run.worker_id)
			: fleet.online.filter((w) => (w.capabilities.repos ?? []).some((r) => repoKey(r) === run.repo || r.name === run.repo))
	);
	const where = $derived(hasFiles ? (run.worker_id ?? 'that machine') : `a machine with ${repoLabel(run.repo)}`);
	const models = $derived(machineModels(machines, agent));
	const can = $derived(!!task.trim() && modelValid(agent, model) && machines.some((w) => hasAgent(w, agent)) && !busy);
	const offline = $derived(hasFiles && !machines.some((w) => w.online));

	async function submit(e: Event) {
		e.preventDefault();
		if (!can) return;
		busy = true;
		try {
			const r = await api<{ id: string }>(`/v1/runs/${run.id}/fork`, {
				method: 'POST',
				body: JSON.stringify({ task: task.trim(), agent, model, permission_mode: perm })
			});
			open = false;
			void fleet.refresh();
			await goto('#/runs/' + r.id);
		} catch (err) {
			toast.error('Could not fork', { description: (err as Error).message });
		}
		busy = false;
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-[36rem]">
		<Dialog.Header>
			<Dialog.Title>Fork into a new thread</Dialog.Title>
			<Dialog.Description>
				A new thread that knows the conversation up to this message{hasFiles ? ', and starts with the files as they were after it, on its own branch' : ''}. This thread stays as it is.
			</Dialog.Description>
		</Dialog.Header>
		<form class="flex flex-col gap-3" onsubmit={submit}>
			<div class="flex flex-wrap items-center gap-1.5">
				<AgentModelPicker
					bind:agent
					bind:model
					{models}
					id="fork-model"
					unavailable={(a) => (machines.some((w) => hasAgent(w, a)) ? '' : `Not on ${where}`)}
					onagent={(a) => (model = a === run.agent ? run.model : defaultModel(a))}
				/>
				<Select.Root type="single" value={perm} onValueChange={(v) => (perm = v)}>
					<Select.Trigger size="sm" class="h-8 rounded-full border-transparent bg-muted text-[13px] shadow-none hover:bg-accent" aria-label="Permissions">{PERMISSIONS.find((p) => p.id === perm)?.name}</Select.Trigger>
					<Select.Content>
						{#each PERMISSIONS as p (p.id)}<Select.Item value={p.id} label={p.name}><span class="flex flex-col"><span>{p.name}</span><span class="text-xs text-muted-foreground">{permHint(p.id, agent)}</span></span></Select.Item>{/each}
					</Select.Content>
				</Select.Root>
			</div>
			<label for="fork-task" class="sr-only">First message of the new thread</label>
			<textarea
				id="fork-task"
				bind:value={task}
				rows="4"
				placeholder="What should {agentName(agent)} do next?"
				onkeydown={(e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) submit(e); }}
				class="w-full resize-none rounded-xl border border-input bg-card px-3.5 py-3 text-[14px] leading-relaxed outline-none placeholder:text-faint focus:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]"
			></textarea>
			<div class="flex items-center gap-3">
				<p class="min-w-0 flex-1 text-xs text-muted-foreground">
					{#if !machines.some((w) => hasAgent(w, agent))}
						<span class="text-err-ink">{agentName(agent)} isn't on {where}.</span>
					{:else}
						Runs with {agentName(agent)} on {where}{offline ? ', once it is back online' : ''}.
					{/if}
				</p>
				<Button type="submit" size="sm" class="rounded-full" disabled={!can}><GitFork class="size-3.5" />{busy ? 'Forking…' : 'Fork'}</Button>
			</div>
		</form>
	</Dialog.Content>
</Dialog.Root>
