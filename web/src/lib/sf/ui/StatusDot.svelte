<script lang="ts">
	import { cn } from '#lib/utils';
	let { status, class: className = '' }: { status: string; class?: string } = $props();
	const color: Record<string, string> = {
		running: 'bg-run pulse-run', queued: 'bg-queue', succeeded: 'bg-ok', failed: 'bg-err',
		stopped: 'bg-err', online: 'bg-ok', offline: 'bg-off'
	};
</script>

<span aria-hidden="true" class={cn('inline-block size-1.5 shrink-0 rounded-full', color[status] ?? 'bg-off', className)}></span>
