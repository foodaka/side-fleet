<script lang="ts">
	// The fleet at a glance: the control plane in the centre, machines on two orbits.
	// Heartbeats arrive as small green dots, a task starting sends a violet dot out,
	// and running tasks circle their machine. Deliberately quiet: motion only means something happened.
	import { onMount } from 'svelte';
	import { fleet } from '../state.svelte';
	import { telemetry, type FleetEvent } from '../telemetry.svelte';
	import { rel } from '../format';
	import type { Worker } from '../types';

	let { hover = $bindable(null) }: { hover?: string | null } = $props();

	let box: HTMLDivElement;
	let canvas: HTMLCanvasElement;
	let beats = $state(0);
	const reduce = typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;

	type Node = { id: string; name: string; worker: Worker | null; ring: 0 | 1 | 2; angle: number; flash: number; phase: number };
	type Pulse = { from: string; to: string; kind: 'beat' | 'task' | 'result'; t: number };
	const HUB = '__control_plane__';

	// Layout: the host machine (or a plain control-plane node) in the centre, the rest spread on two orbits.
	const nodes = $derived.by((): Node[] => {
		const hostId = fleet.setup?.host_worker ?? null;
		const host = fleet.workers.find((w) => w.id === hostId) ?? null;
		const others = fleet.workers.filter((w) => w !== host).sort((a, b) => a.display_name.localeCompare(b.display_name));
		const preset: Record<number, number[]> = { 1: [-1.57], 2: [-2.45, -0.62], 3: [-2.45, -0.62, 1.57] };
		const angles = preset[others.length] ?? others.map((_, i) => -2.45 + (i * Math.PI * 2) / others.length);
		return [
			{ id: host?.id ?? HUB, name: host?.display_name ?? 'Control plane', worker: host, ring: 0, angle: 0, flash: 0, phase: 0 },
			...others.map((w, i) => ({ id: w.id, name: w.display_name, worker: w, ring: (i % 2 ? 2 : 1) as 1 | 2, angle: angles[i], flash: 0, phase: i * 1.7 }))
		];
	});
	// Flash state survives layout recomputes.
	const flashes = new Map<string, number>();
	const pulses: Pulse[] = [];

	const running = (id: string) => fleet.runs.filter((r) => r.thread_status === 'running' && r.worker_id === id).length;
	const hubId = () => nodes[0]?.id ?? HUB;

	function onEvent(e: FleetEvent) {
		const hub = hubId();
		if (e.kind === 'beat') {
			beats++;
			if (e.worker === hub) flashes.set(hub, 0.6);
			else pulses.push({ from: e.worker, to: hub, kind: 'beat', t: 0 });
		} else if (e.kind === 'dispatch') {
			if (e.worker === hub) flashes.set(hub, 1);
			else pulses.push({ from: hub, to: e.worker, kind: 'task', t: 0 });
		} else if (e.worker !== hub) {
			pulses.push({ from: e.worker, to: hub, kind: 'result', t: 0 });
		}
	}

	let W = 0, H = 0;
	let colors: Record<string, string> = {};
	function readColors() {
		const cs = getComputedStyle(document.documentElement);
		const v = (n: string) => cs.getPropertyValue(n).trim();
		colors = { ink: v('--foreground'), muted: v('--muted-foreground'), faint: v('--faint'), ok: v('--ok'), run: v('--run'), warn: v('--queue'), line: v('--border'), card: v('--card'), off: v('--off'), font: cs.getPropertyValue('--font-sans').trim() || 'system-ui' };
	}
	function resize() {
		const r = canvas.getBoundingClientRect();
		const dpr = Math.min(2, window.devicePixelRatio || 1);
		W = r.width; H = r.height;
		canvas.width = Math.round(W * dpr); canvas.height = Math.round(H * dpr);
		canvas.getContext('2d')!.setTransform(dpr, 0, 0, dpr, 0, 0);
	}

	const wide = () => W / Math.max(H, 1) > 1.6;
	const ringR = (i: number) => Math.min(W, H) * [0, 0.27, 0.4][i];
	function pos(n: Node) {
		const cx = W / 2, cy = H / 2;
		if (n.ring === 0) return { x: cx, y: cy };
		return { x: cx + Math.cos(n.angle) * ringR(n.ring) * (wide() ? 1.7 : 1.15), y: cy + Math.sin(n.angle) * ringR(n.ring) };
	}
	const nodeR = (n: Node) => 10 + Math.sqrt(n.worker?.capabilities.system?.cpu_cores ?? 4) * 3;

	function frame(time: number) {
		const ctx = canvas.getContext('2d')!;
		ctx.clearRect(0, 0, W, H);
		const list = nodes;
		const byId = new Map(list.map((n) => [n.id, n]));
		const hub = list[0];
		if (!hub) return;
		const c = pos(hub);

		// the two orbits
		ctx.lineWidth = 1;
		ctx.strokeStyle = colors.line;
		[1, 2].forEach((i) => {
			if (!list.some((n) => n.ring === i)) return;
			ctx.beginPath();
			ctx.ellipse(c.x, c.y, ringR(i) * (wide() ? 1.7 : 1.15), ringR(i), 0, 0, Math.PI * 2);
			ctx.stroke();
		});

		// links
		for (const n of list) {
			if (n.ring === 0) continue;
			const p = pos(n);
			const on = !!n.worker?.online;
			ctx.strokeStyle = hover === n.id ? colors.run : colors.line;
			ctx.globalAlpha = on ? 1 : 0.5;
			ctx.lineWidth = hover === n.id ? 1.5 : 1;
			ctx.setLineDash([3, 5]);
			ctx.beginPath(); ctx.moveTo(c.x, c.y); ctx.lineTo(p.x, p.y); ctx.stroke();
			ctx.setLineDash([]);
			ctx.globalAlpha = 1;
		}

		// dots travelling along links
		for (let i = pulses.length - 1; i >= 0; i--) {
			const pu = pulses[i];
			const a = byId.get(pu.from), b = byId.get(pu.to);
			if (!a || !b) { pulses.splice(i, 1); continue; }
			pu.t += reduce ? 1 : pu.kind === 'beat' ? 0.011 : 0.013;
			const t = Math.min(1, pu.t), e = 1 - Math.pow(1 - t, 2);
			const pa = pos(a), pb = pos(b);
			ctx.fillStyle = pu.kind === 'task' ? colors.run : colors.ok;
			ctx.globalAlpha = pu.kind === 'beat' ? 0.5 : 0.85;
			ctx.beginPath(); ctx.arc(pa.x + (pb.x - pa.x) * e, pa.y + (pb.y - pa.y) * e, pu.kind === 'beat' ? 2.2 : 3.2, 0, Math.PI * 2); ctx.fill();
			ctx.globalAlpha = 1;
			if (pu.t >= 1) {
				pulses.splice(i, 1);
				flashes.set(b.id, Math.max(flashes.get(b.id) ?? 0, pu.kind === 'beat' ? 0.6 : 1));
			}
		}

		// machines
		for (const n of list) {
			const p = pos(n), r = nodeR(n);
			const w = n.worker, on = n.ring === 0 && !w ? true : !!w?.online;
			const f = flashes.get(n.id) ?? 0;
			if (f > 0) {
				ctx.strokeStyle = running(n.id) ? colors.run : colors.ok;
				ctx.globalAlpha = f * 0.3;
				ctx.lineWidth = 1.5;
				ctx.beginPath(); ctx.arc(p.x, p.y, r + 4 + (1 - f) * 8, 0, Math.PI * 2); ctx.stroke();
				ctx.globalAlpha = 1;
				flashes.set(n.id, Math.max(0, f - 0.025));
			}
			ctx.globalAlpha = on ? 1 : 0.55;
			ctx.fillStyle = colors.card;
			ctx.strokeStyle = hover === n.id ? colors.run : colors.line;
			ctx.lineWidth = hover === n.id ? 2 : 1;
			ctx.beginPath(); ctx.arc(p.x, p.y, r, 0, Math.PI * 2); ctx.fill(); ctx.stroke();
			const cpu = w?.capabilities.system?.cpu_percent;
			if (on && cpu != null) {
				ctx.strokeStyle = cpu > 75 ? colors.warn : colors.ink;
				ctx.lineWidth = 2.5; ctx.lineCap = 'round';
				ctx.beginPath(); ctx.arc(p.x, p.y, r - 4.5, -Math.PI / 2, -Math.PI / 2 + (Math.PI * 2 * Math.max(cpu, 1)) / 100); ctx.stroke();
				ctx.lineCap = 'butt';
			}
			if (n.ring === 0) {
				ctx.fillStyle = colors.ink;
				[[-4, 0], [3.2, -3.6], [3.2, 3.6]].forEach(([dx, dy]) => { ctx.beginPath(); ctx.arc(p.x + dx, p.y + dy, 2.4, 0, Math.PI * 2); ctx.fill(); });
			}
			const busy = running(n.id);
			for (let i = 0; i < busy; i++) {
				const ang = n.phase + (reduce ? 0 : time / 2600) + (i * Math.PI * 2) / busy;
				ctx.fillStyle = colors.run;
				ctx.beginPath(); ctx.arc(p.x + Math.cos(ang) * (r + 7), p.y + Math.sin(ang) * (r + 7), 2.6, 0, Math.PI * 2); ctx.fill();
			}
			ctx.textAlign = 'center';
			ctx.font = `600 13px ${colors.font}`;
			ctx.fillStyle = colors.ink;
			ctx.fillText(n.name, p.x, p.y + r + 30);
			ctx.font = `400 11.5px ${colors.font}`;
			ctx.fillStyle = colors.muted;
			let sub = !w ? 'control plane' : !w.online ? `offline, seen ${rel(w.last_heartbeat_at, fleet.now)}` : `${busy ? busy + ' running' : 'idle'}${cpu != null ? `, CPU ${Math.round(cpu)}%` : ''}`;
			if (n.ring === 0 && w) sub = 'control plane, ' + sub;
			ctx.fillText(sub, p.x, p.y + r + 45);
			ctx.globalAlpha = 1;
		}
	}

	onMount(() => {
		readColors();
		resize();
		const ro = new ResizeObserver(resize);
		ro.observe(canvas);
		const mq = matchMedia('(prefers-color-scheme: dark)');
		const mo = new MutationObserver(() => requestAnimationFrame(readColors));
		mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
		mq.addEventListener('change', readColors);
		const off = telemetry.on(onEvent);
		let raf = 0;
		const loop = (t: number) => { frame(t); raf = requestAnimationFrame(loop); };
		raf = requestAnimationFrame(loop);
		return () => { cancelAnimationFrame(raf); ro.disconnect(); mo.disconnect(); mq.removeEventListener('change', readColors); off(); };
	});

	function pick(e: MouseEvent) {
		const r = canvas.getBoundingClientRect();
		const x = e.clientX - r.left, y = e.clientY - r.top;
		const hit = nodes.find((n) => { const p = pos(n); return Math.hypot(p.x - x, p.y - y) < nodeR(n) + 10; });
		hover = hit && hit.worker ? hit.id : null;
		canvas.style.cursor = hover ? 'pointer' : 'default';
	}
	function open() {
		if (!hover) return;
		document.getElementById('machine-' + hover)?.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'nearest' });
	}
</script>

<div bind:this={box} class="relative h-[clamp(220px,26vw,290px)] overflow-hidden rounded-2xl border bg-card" aria-label="Fleet map: the control plane in the centre and each connected machine around it">
	<canvas bind:this={canvas} class="absolute inset-0 size-full" onmousemove={pick} onmouseleave={() => (hover = null)} onclick={open}></canvas>
	<p class="pointer-events-none absolute top-3 right-4 text-xs text-muted-foreground"><span class="tnum font-medium text-foreground">{beats}</span> heartbeats while you watched</p>
	<div class="pointer-events-none absolute bottom-3 left-4 flex gap-4 text-xs text-muted-foreground" aria-hidden="true">
		<span class="inline-flex items-center gap-1.5"><span class="size-2 rounded-full bg-ok"></span>Heartbeat</span>
		<span class="inline-flex items-center gap-1.5"><span class="size-2 rounded-full bg-run"></span>Task</span>
	</div>
</div>
