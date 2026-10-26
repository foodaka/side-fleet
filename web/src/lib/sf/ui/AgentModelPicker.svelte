<script lang="ts">
	// The agent menu and, next to it, the model control that suits that agent:
	// Claude's model aliases, the machine's Codex models, or a searchable field for opencode;
	// then the reasoning effort, for the agents that take one.
	import * as Select from '#lib/components/ui/select';
	import GaugeIcon from '@lucide/svelte/icons/gauge';
	import { AGENTS, EFFORTS, MODELS, agentName, effortName, effortValid, modelName, modelValid } from '../format';
	import type { AgentId } from '../types';
	import AgentIcon from './AgentIcon.svelte';

	let {
		agent = $bindable(),
		model = $bindable(),
		effort = $bindable(''),
		models,
		unavailable = () => '',
		onagent,
		onmodel,
		oneffort,
		id = 'model',
		size = 'default'
	}: {
		agent: AgentId;
		model: string;
		/** Reasoning effort; '' is the agent's default. */
		effort?: string;
		/** Models the target machines offer for the agent (Codex, opencode). */
		models: string[];
		/** Why an agent can't be picked, or '' when it can. */
		unavailable?: (a: AgentId) => string;
		onagent?: (a: AgentId) => void;
		onmodel?: (m: string) => void;
		oneffort?: (e: string) => void;
		id?: string;
		/** `sm` to sit with the reply box's smaller controls. */
		size?: 'default' | 'sm';
	} = $props();

	const efforts = $derived(EFFORTS[agent]);

	const ok = $derived(modelValid(agent, model));
	// Quiet controls: no fill at rest, the same hover as the composer's other buttons.
	const pill = $derived(`${size === 'sm' ? 'h-7' : 'h-8'} rounded-full border-transparent bg-transparent px-2.5 shadow-none hover:bg-accent dark:bg-transparent dark:hover:bg-accent data-[state=open]:bg-accent [&>svg:last-child]:size-3.5 [&>svg:last-child]:opacity-60`);
	const text = $derived(size === 'sm' ? 'text-xs' : 'text-[13px]');
	const mono = $derived(size === 'sm' ? 'text-[11.5px]' : 'text-[12.5px]');

	function pickAgent(v: string) {
		agent = v as AgentId;
		// Efforts differ per agent; the parent may pick a remembered one in onagent.
		if (!effortValid(agent, effort)) effort = '';
		onagent?.(agent);
	}
	function pickModel(v: string) {
		model = v.trim();
		onmodel?.(model);
	}
	function pickEffort(v: string) {
		effort = v;
		oneffort?.(effort);
	}
</script>

<Select.Root type="single" value={agent} onValueChange={pickAgent}>
	<Select.Trigger size="sm" class="{pill} {text}" aria-label="Agent"><AgentIcon {agent} />{agentName(agent)}</Select.Trigger>
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
		<Select.Trigger size="sm" class="{pill} {text}" aria-label="Model">{modelName(model)}</Select.Trigger>
		<Select.Content>
			{#each MODELS as m (m.id)}<Select.Item value={m.id} label={m.name}><span class="flex flex-col"><span>{m.name}</span><span class="text-xs text-muted-foreground">{m.hint}</span></span></Select.Item>{/each}
		</Select.Content>
	</Select.Root>
{:else if agent === 'codex' && (models.length || !model)}
	<Select.Root type="single" value={model} onValueChange={pickModel}>
		<Select.Trigger size="sm" class="{pill} font-mono {mono}" aria-label="Model">{model || 'Default model'}</Select.Trigger>
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
		class="{size === 'sm' ? 'h-7 w-[13rem]' : 'h-8 w-[15rem]'} max-w-full rounded-full bg-transparent px-3 font-mono {mono} outline-none placeholder:text-muted-foreground hover:bg-accent focus:bg-accent {ok ? '' : 'text-err-ink'}"
	/>
	<datalist id="{id}-options">{#each models as m (m)}<option value={m}></option>{/each}</datalist>
{/if}

{#if efforts.length}
	<Select.Root type="single" value={effort} onValueChange={pickEffort}>
		<Select.Trigger size="sm" class="{pill} {text}" aria-label="Reasoning effort"><GaugeIcon class="size-3.5 text-muted-foreground" />{#if effort}{effortName(agent, effort)}{:else}<span class="text-muted-foreground">Effort</span>{/if}</Select.Trigger>
		<Select.Content>
			<Select.Group>
				<Select.GroupHeading>Reasoning</Select.GroupHeading>
				<Select.Item value="" label="Default"><span class="flex flex-col"><span>Default</span><span class="text-xs text-muted-foreground">Whatever {agentName(agent)} uses on that machine</span></span></Select.Item>
				{#each efforts as e (e.id)}
					<Select.Item value={e.id} label={e.name}>{#if e.hint}<span class="flex flex-col"><span>{e.name}</span><span class="text-xs text-muted-foreground">{e.hint}</span></span>{:else}{e.name}{/if}</Select.Item>
				{/each}
			</Select.Group>
		</Select.Content>
	</Select.Root>
{/if}
