<script lang="ts">
	import { cn } from '#lib/utils';
	import { STATUS_LABEL } from '../format';
	import StatusDot from './StatusDot.svelte';
	let { status, extra = '', class: className = '' }: { status: string; extra?: string; class?: string } = $props();
	const tone: Record<string, string> = {
		running: 'bg-run-soft text-run-ink', queued: 'bg-queue-soft text-queue-ink', succeeded: 'bg-ok-soft text-ok-ink',
		failed: 'bg-err-soft text-err-ink', stopped: 'bg-err-soft text-err-ink'
	};
</script>

<span class={cn('inline-flex h-6 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium whitespace-nowrap', tone[status] ?? 'bg-muted text-muted-foreground', className)}>
	<StatusDot {status} />{STATUS_LABEL[status] ?? status}{#if extra}<span class="tnum opacity-75">{extra}</span>{/if}
</span>
