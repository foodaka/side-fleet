<script lang="ts">
	// Settings: the services a task's agent may pay without asking, picked from the MPP catalog.
	import { toast } from 'svelte-sonner';
	import X from '@lucide/svelte/icons/x';
	import Plus from '@lucide/svelte/icons/plus';
	import { catalog, getPolicy, savePolicy, type CatalogService, type PayPolicy } from '../services';

	let policy = $state<PayPolicy | null>(null);
	let services = $state<CatalogService[]>([]);
	let query = $state('');

	$effect(() => {
		getPolicy().then((p) => (policy = p)).catch(() => (policy = { mode: 'approved', approved: [] }));
		catalog().then((s) => (services = s)).catch(() => (services = []));
	});

	const approvedHosts = $derived(new Set(policy?.approved.map((s) => s.host) ?? []));
	const matches = $derived.by(() => {
		const q = query.trim().toLowerCase();
		if (!q) return [];
		return services
			.filter((s) => !approvedHosts.has(s.host) && [s.name, s.host, s.id, ...(s.categories ?? [])].some((f) => f?.toLowerCase().includes(q)))
			.slice(0, 6);
	});
	/** A host typed in that the catalog doesn't have. */
	const typedHost = $derived(/^[a-z0-9-]+(\.[a-z0-9-]+)+$/i.test(query.trim()) && !matches.some((s) => s.host === query.trim().toLowerCase()) ? query.trim().toLowerCase() : null);

	async function save(next: PayPolicy) {
		const before = policy;
		policy = next;
		try {
			policy = await savePolicy(next);
		} catch (e) {
			policy = before;
			toast.error('Could not save your approved services', { description: (e as Error).message });
		}
	}
	const add = (host: string, name?: string) => {
		if (!policy) return;
		query = '';
		void save({ ...policy, approved: [...policy.approved, { host, name }] });
	};
	const remove = (host: string) => policy && save({ ...policy, approved: policy.approved.filter((s) => s.host !== host) });
	const short = (a: string) => `${a.slice(0, 6)}…${a.slice(-4)}`;
</script>

<div class="flex flex-col gap-3">
	<div>
		<h2 class="text-[15px] font-medium">Approved services</h2>
		<p class="mt-0.5 text-[12.5px] text-muted-foreground">
			Who a task's agent may pay without asking. Each service's first payment pins the address it was paid to; if it later asks to be paid elsewhere, you're asked first.
		</p>
	</div>
	{#if policy}
		<div class="divide-y rounded-xl border bg-card">
			<fieldset class="flex flex-col gap-2 p-4">
				<legend class="sr-only">Other services</legend>
				<label class="flex items-start gap-2.5 text-[13.5px]">
					<input type="radio" name="pay-mode" class="mt-1" checked={policy.mode === 'approved'} onchange={() => policy && save({ ...policy, mode: 'approved' })} />
					<span><span class="font-medium">Ask before paying any other service</span><span class="block text-[12.5px] text-muted-foreground">Recommended. The approval offers to add it here.</span></span>
				</label>
				<label class="flex items-start gap-2.5 text-[13.5px]">
					<input type="radio" name="pay-mode" class="mt-1" checked={policy.mode === 'any'} onchange={() => policy && save({ ...policy, mode: 'any' })} />
					<span><span class="font-medium">Pay any service</span><span class="block text-[12.5px] text-muted-foreground">Up to each task's threshold, without asking.</span></span>
				</label>
			</fieldset>
			{#each policy.approved as s (s.host)}
				<div class="flex items-center gap-3 px-4 py-2.5">
					<div class="flex min-w-0 flex-1 flex-col">
						<span class="truncate text-[13.5px]">{s.name ?? s.host}</span>
						<span class="truncate font-mono text-[11.5px] text-muted-foreground">{[s.name ? s.host : null, s.recipient ? `pays ${short(s.recipient)}` : 'not paid yet'].filter(Boolean).join(' · ')}</span>
					</div>
					<button type="button" aria-label="Remove {s.name ?? s.host}" onclick={() => remove(s.host)} class="grid size-8 place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground"><X class="size-4" /></button>
				</div>
			{/each}
			<div class="p-3">
				<label for="add-service" class="sr-only">Add a service</label>
				<input id="add-service" type="search" bind:value={query} placeholder="Add a service: search {services.length || ''} from the MPP catalog, or type a host" class="h-9 w-full rounded-lg border bg-background px-3 text-[13px] outline-none focus:ring-1 focus:ring-ring" />
				{#if matches.length || typedHost}
					<ul class="mt-1.5 flex flex-col">
						{#each matches as s (s.id)}
							<li>
								<button type="button" onclick={() => add(s.host, s.name)} class="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left hover:bg-accent">
									<Plus class="size-3.5 shrink-0 text-muted-foreground" />
									<span class="min-w-0 flex-1 truncate text-[13px]">{s.name} <span class="font-mono text-[11.5px] text-muted-foreground">{s.host}</span></span>
									{#if s.categories?.length}<span class="shrink-0 text-[11.5px] text-muted-foreground">{s.categories.slice(0, 2).join(', ')}</span>{/if}
								</button>
							</li>
						{/each}
						{#if typedHost}
							<li>
								<button type="button" onclick={() => typedHost && add(typedHost)} class="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left hover:bg-accent">
									<Plus class="size-3.5 shrink-0 text-muted-foreground" /><span class="text-[13px]">Add <span class="font-mono">{typedHost}</span></span>
								</button>
							</li>
						{/if}
					</ul>
				{/if}
			</div>
		</div>
	{/if}
</div>
