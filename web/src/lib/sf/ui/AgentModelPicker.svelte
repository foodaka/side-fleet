<script lang="ts">
	// The agent menu and, next to it, the model control that suits that agent:
	// Claude's three models, the machine's Codex models, or a searchable field for opencode.
	import * as Select from '#lib/components/ui/select';
	import { AGENTS, MODELS, agentName, modelName, modelValid } from '../format';
	import type { AgentId } from '../types';
	import AgentIcon from './AgentIcon.svelte';

	let {
		agent = $bindable(),
		model = $bindable(),
		models,
		unavailable = () => '',
		onagent,
		onmodel,
		id = 'model'
	}: {
		agent: AgentId;
		model: string;
		/** Models the target machines offer for the agent (Codex, opencode). */
		models: string[];
		/** Why an agent can't be picked, or '' when it can. */
		unavailable?: (a: AgentId) => string;
		onagent?: (a: AgentId) => void;
		onmodel?: (m: string) => void;
		id?: string;
	} = $props();

	const ok = $derived(modelValid(agent, model));
	const pill = 'h-8 rounded-full border-transparent bg-muted shadow-none hover:bg-accent';

	function pickAgent(v: string) {
		agent = v as AgentId;
		onagent?.(agent);
	}
	function pickModel(v: string) {
		model = v.trim();
		onmodel?.(model);
	}
</script>

<Select.Root type="single" value={agent} onValueChange={pickAgent}>
	<Select.Trigger size="sm" class="{pill} text-[13px]" aria-label="Agent"><AgentIcon {agent} />{agentName(agent)}</Select.Trigger>
	<Select.Content>
		{#each AGENTS as a (a.id)}
			{@const why = unavailable(a.id)}
			<Select.Item value={a.id} label={a.name} disabled={!!why}>
				<AgentIcon agent={a.id} class="mt-0.5 self-start" /><span class="flex flex-col"><span>{a.name}</span><span class="text-xs text-muted-foreground">{why || a.hint}</span></span>
			</Select.Item>
		{/each}
	</Select.Content>
</Select.Root>

{#if agent === 'claude'}
	<Select.Root type="single" value={model} onValueChange={pickModel}>
		<Select.Trigger size="sm" class="{pill} text-[13px]" aria-label="Model">{modelName(model)}</Select.Trigger>
		<Select.Content>
			{#each MODELS as m (m.id)}<Select.Item value={m.id} label={m.name}><span class="flex flex-col"><span>{m.name}</span><span class="text-xs text-muted-foreground">{m.hint}</span></span></Select.Item>{/each}
		</Select.Content>
	</Select.Root>
{:else if agent === 'codex' && (models.length || !model)}
	<Select.Root type="single" value={model} onValueChange={pickModel}>
		<Select.Trigger size="sm" class="{pill} font-mono text-[12.5px]" aria-label="Model">{model || 'Default model'}</Select.Trigger>
		<Select.Content>
			<Select.Item value="" label="Default model"><span class="flex flex-col"><span>Default model</span><span class="text-xs text-muted-foreground">Whatever Codex uses on that machine</span></span></Select.Item>
			{#if models.length}<Select.Separator />{/if}
			{#each models as m (m)}<Select.Item value={m} label={m}><span class="font-mono text-[12.5px]">{m}</span></Select.Item>{/each}
			{#if model && !models.includes(model)}<Select.Item value={model} label={model}><span class="font-mono text-[12.5px]">{model}</span></Select.Item>{/if}
		</Select.Content>
	</Select.Root>
{:else}
	<!-- opencode can offer hundreds of models (OpenRouter alone): type to search them. -->
	<label class="sr-only" for={id}>Model</label>
	<input
		{id}
		list="{id}-options"
		value={model}
		oninput={(e) => (model = e.currentTarget.value.trim())}
		onchange={(e) => pickModel(e.currentTarget.value)}
		placeholder="Default model"
		spellcheck="false"
		autocomplete="off"
		class="h-8 w-[15rem] max-w-full rounded-full bg-muted px-3 font-mono text-[12.5px] outline-none placeholder:text-muted-foreground hover:bg-accent focus:bg-accent {ok ? '' : 'text-err-ink'}"
	/>
	<datalist id="{id}-options">{#each models as m (m)}<option value={m}></option>{/each}</datalist>
{/if}
