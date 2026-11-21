<script lang="ts">
	// The composer's less-used settings behind one control: machine, permissions, workspace and
	// failover. At their defaults it is just an icon; anything changed is named on the control.
	import { goto } from '$app/navigation';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
	import { ASK_ABOVE, BUDGETS, WORKSPACES, permHint, permName, permsFor, usd } from '../format';
	import type { AgentId, Worker } from '../types';

	let {
		agent,
		perm,
		onperm,
		defaultPerm = 'acceptEdits',
		defaultIso = 'worktree',
		sameAs,
		machines = [],
		machine = '',
		onmachine,
		iso,
		oniso,
		failover = $bindable(false),
		budget = $bindable(),
		askAbove = $bindable(1_000_000),
		size = 'default'
	}: {
		agent: AgentId;
		/** The permission mode; '' means "same as before" when `sameAs` is given. */
		perm: string;
		onperm?: (p: string) => void;
		/** The mode that counts as unchanged: your default from Settings. */
		defaultPerm?: string;
		/** The workspace that counts as unchanged. */
		defaultIso?: string;
		/** The reply box: the previous message's mode, offered as "Same permissions". */
		sameAs?: string | null;
		/** Machines to choose from; the section shows only when there is a choice. */
		machines?: Worker[];
		/** The pinned machine's id, '' for any. */
		machine?: string;
		onmachine?: (id: string) => void;
		/** The workspace; leave out to hide the section. */
		iso?: string;
		oniso?: (w: string) => void;
		/** "Move if it drops"; only offered on its own branch. */
		failover?: boolean;
		/** Agent budgets (Claude Code, Codex): the most the agent may pay for APIs, in micros; 0 is none. Leave out to hide. */
		budget?: number;
		/** Payments above this ask first. */
		askAbove?: number;
		size?: 'default' | 'sm';
	} = $props();

	const ANY = '__any__';
	const SAME = '__same__';
	const pinned = $derived(machines.find((w) => w.id === machine));
	const showMachines = $derived(machines.length > 1);
	const showFailover = $derived(iso === 'worktree');
	// opencode can't be given the pay tool yet (see check_budget on the server).
	const showBudget = $derived(agent !== 'opencode' && budget !== undefined);

	/** What differs from the defaults, in the order the menu lists it. */
	const changed = $derived(
		[
			showMachines && pinned ? pinned.display_name : '',
			perm && perm !== defaultPerm ? permName(perm) : '',
			iso && iso !== defaultIso ? WORKSPACES.find((w) => w.id === iso)?.name : '',
			showFailover && failover ? 'Moves if it drops' : '',
			showBudget && budget ? `Budget ${usd(budget)}` : ''
		].filter(Boolean)
	);
	const summary = $derived(changed.join(' · '));
	const label = $derived(
		[showMachines && 'machine', 'permissions', iso && 'workspace'].filter(Boolean).join(', ').replace(/^./, (c) => c.toUpperCase())
	);

	const text = $derived(size === 'sm' ? 'text-xs' : 'text-[13px]');
	const hint = 'text-xs text-muted-foreground';
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger
		class="inline-flex {size === 'sm' ? 'h-7' : 'h-8'} min-w-0 items-center gap-1.5 rounded-full {summary ? 'px-2.5' : 'w-8 justify-center'} {text} text-muted-foreground transition-colors hover:bg-accent hover:text-foreground data-[state=open]:bg-accent data-[state=open]:text-foreground"
		aria-label="{label}{summary ? `: ${summary}` : ''}"
		title={label}
	>
		<SlidersHorizontal class="size-3.5 shrink-0" />{#if summary}<span class="truncate text-foreground/80">{summary}</span>{/if}
	</DropdownMenu.Trigger>
	<DropdownMenu.Content align="start" class="w-64">
		{#if showMachines}
			<DropdownMenu.Sub>
				<DropdownMenu.SubTrigger><span class="flex-1">Machine</span><span class="max-w-28 truncate {hint}">{pinned?.display_name ?? 'Any'}</span></DropdownMenu.SubTrigger>
				<DropdownMenu.SubContent class="w-60">
					<DropdownMenu.RadioGroup value={pinned?.id ?? ANY} onValueChange={(v) => onmachine?.(v === ANY ? '' : v)}>
						<DropdownMenu.RadioItem value={ANY} closeOnSelect><span class="flex flex-col"><span>Any machine</span><span class={hint}>The first one with a free slot</span></span></DropdownMenu.RadioItem>
						<DropdownMenu.Separator />
						{#each machines as w (w.id)}
							<DropdownMenu.RadioItem value={w.id} closeOnSelect>
								<span class="flex flex-col"><span>{w.display_name}</span>{#if w.capabilities.slots}<span class="tnum {hint}">{w.capabilities.slots.active} of {w.capabilities.slots.parallel} slots busy</span>{/if}</span>
							</DropdownMenu.RadioItem>
						{/each}
					</DropdownMenu.RadioGroup>
				</DropdownMenu.SubContent>
			</DropdownMenu.Sub>
		{/if}

		<DropdownMenu.Sub>
			<DropdownMenu.SubTrigger><span class="flex-1">Permissions</span><span class="max-w-28 truncate {hint}">{perm ? permName(perm) : 'Same'}</span></DropdownMenu.SubTrigger>
			<DropdownMenu.SubContent class="w-72">
				<DropdownMenu.RadioGroup value={perm || SAME} onValueChange={(v) => onperm?.(v === SAME ? '' : v)}>
					{#if sameAs !== undefined}
						<DropdownMenu.RadioItem value={SAME} closeOnSelect><span class="flex flex-col"><span>Same permissions</span>{#if sameAs}<span class={hint}>{permName(sameAs)}, as the last message</span>{/if}</span></DropdownMenu.RadioItem>
						<DropdownMenu.Separator />
					{/if}
					{#each permsFor(agent) as p (p.id)}
						<DropdownMenu.RadioItem value={p.id} closeOnSelect><span class="flex flex-col"><span>{p.name}</span><span class={hint}>{permHint(p.id, agent)}</span></span></DropdownMenu.RadioItem>
					{/each}
				</DropdownMenu.RadioGroup>
			</DropdownMenu.SubContent>
		</DropdownMenu.Sub>

		{#if iso}
			<DropdownMenu.Sub>
				<DropdownMenu.SubTrigger><span class="flex-1">Workspace</span><span class={hint}>{WORKSPACES.find((w) => w.id === iso)?.name}</span></DropdownMenu.SubTrigger>
				<DropdownMenu.SubContent class="w-72">
					<DropdownMenu.RadioGroup value={iso} onValueChange={(v) => oniso?.(v)}>
						{#each WORKSPACES as w (w.id)}
							<DropdownMenu.RadioItem value={w.id} closeOnSelect><span class="flex flex-col"><span>{w.name}</span><span class={hint}>{w.hint}</span></span></DropdownMenu.RadioItem>
						{/each}
					</DropdownMenu.RadioGroup>
				</DropdownMenu.SubContent>
			</DropdownMenu.Sub>
		{/if}

		{#if showBudget}
			<DropdownMenu.Separator />
			<DropdownMenu.Sub>
				<DropdownMenu.SubTrigger><span class="flex-1">Budget</span><span class={hint}>{budget ? usd(budget) : 'None'}</span></DropdownMenu.SubTrigger>
				<DropdownMenu.SubContent class="w-72">
					<DropdownMenu.RadioGroup value={String(budget)} onValueChange={(v) => (budget = Number(v))}>
						{#each BUDGETS as b (b)}
							<DropdownMenu.RadioItem value={String(b)} closeOnSelect>
								<span class="flex flex-col"><span>{b ? usd(b) : 'No budget'}</span><span class={hint}>{b ? `The agent may pay for APIs up to ${usd(b)}` : 'The agent pays for nothing'}</span></span>
							</DropdownMenu.RadioItem>
						{/each}
					</DropdownMenu.RadioGroup>
				</DropdownMenu.SubContent>
			</DropdownMenu.Sub>
			{#if budget}
				<DropdownMenu.Sub>
					<DropdownMenu.SubTrigger><span class="flex-1">Ask me over</span><span class={hint}>{usd(askAbove)}</span></DropdownMenu.SubTrigger>
					<DropdownMenu.SubContent class="w-64">
						<DropdownMenu.RadioGroup value={String(askAbove)} onValueChange={(v) => (askAbove = Number(v))}>
							{#each ASK_ABOVE as a (a)}
								<DropdownMenu.RadioItem value={String(a)} closeOnSelect>{a ? usd(a) : 'Every payment'}</DropdownMenu.RadioItem>
							{/each}
						</DropdownMenu.RadioGroup>
					</DropdownMenu.SubContent>
				</DropdownMenu.Sub>
			{/if}
		{/if}

		{#if showFailover}
			<DropdownMenu.Separator />
			<DropdownMenu.CheckboxItem bind:checked={failover}>
				<span class="flex flex-col"><span>Move if it drops</span><span class={hint}>Pushes checkpoints of the task's branch to origin, so another machine can take over</span></span>
			</DropdownMenu.CheckboxItem>
		{/if}

		{#if sameAs === undefined}
			<DropdownMenu.Separator />
			<DropdownMenu.Item onclick={() => goto('#/settings')}><span class={hint}>Changes here are for this task only. Set your defaults in Settings…</span></DropdownMenu.Item>
		{/if}
	</DropdownMenu.Content>
</DropdownMenu.Root>
