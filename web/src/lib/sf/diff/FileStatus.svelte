<script lang="ts">
	// GitHub's file status squares: added, deleted, modified, renamed.
	import SquarePlus from '@lucide/svelte/icons/square-plus';
	import SquareMinus from '@lucide/svelte/icons/square-minus';
	import SquareDot from '@lucide/svelte/icons/square-dot';
	import SquareArrowRight from '@lucide/svelte/icons/square-arrow-right';
	import type { DiffFile } from './parse';

	let { status, class: className = 'size-3.5' }: { status: DiffFile['status']; class?: string } = $props();
	const look = {
		added: { icon: SquarePlus, tone: 'text-diff-add-ink', label: 'Added' },
		deleted: { icon: SquareMinus, tone: 'text-diff-del-ink', label: 'Deleted' },
		modified: { icon: SquareDot, tone: 'text-queue', label: 'Modified' },
		renamed: { icon: SquareArrowRight, tone: 'text-muted-foreground', label: 'Renamed' }
	};
	const l = $derived(look[status] ?? look.modified);
</script>

<l.icon class="shrink-0 {l.tone} {className}" aria-label={l.label} />
