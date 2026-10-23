/*
 * Fibres: bundles of thin coloured strands drawn on a canvas behind a section, with
 * bright pulses that travel along them. A bundle is a cubic Bézier centreline; each
 * strand sits at its own offset along the normal, scaled by the bundle's shape so
 * the strands pinch together at the ends (or fan out from one point), and sways a
 * little over time. Pulses are short gradients that run from one end to the other.
 *
 * Fibres.mount(host, build) adds a canvas to host. build(rect) returns the bundles,
 * in host coordinates, and is called again whenever the host resizes. The returned
 * controller can fire(id) a pulse (a promise that resolves when it arrives) and
 * light(id) one bundle so the others fade back.
 */
(() => {
	const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
	const dark = matchMedia('(prefers-color-scheme: dark)');
	const HUES = ['#5c98ff', '#8b5cf6', '#ff6fae', '#f08a2e', '#13a05a', '#3fb8c9'];
	const SAMPLES = 72;
	const hosts = new Set();
	let frame = 0;

	function bez(a, b, c, d, t) {
		const u = 1 - t;
		return u * u * u * a + 3 * u * u * t * b + 3 * u * t * t * c + t * t * t * d;
	}

	// Seeded noise per strand so layouts are stable between resizes.
	function rand(i) {
		const x = Math.sin(i * 127.1 + 311.7) * 43758.5453;
		return x - Math.floor(x);
	}

	function envelope(shape, t) {
		if (shape === 'fan') return Math.pow(t, 0.85);
		if (shape === 'flat') return 0.35 + 0.65 * Math.sin(Math.PI * t);
		return Math.pow(Math.sin(Math.PI * t), 0.75);
	}

	function prepare(b, k) {
		const p = [b.from, b.c1, b.c2, b.to];
		const pts = [];
		for (let s = 0; s <= SAMPLES; s++) {
			const t = s / SAMPLES;
			pts.push([bez(p[0].x, p[1].x, p[2].x, p[3].x, t), bez(p[0].y, p[1].y, p[2].y, p[3].y, t)]);
		}
		const normals = pts.map((_, s) => {
			const a = pts[Math.max(0, s - 1)], c = pts[Math.min(SAMPLES, s + 1)];
			const dx = c[0] - a[0], dy = c[1] - a[1], len = Math.hypot(dx, dy) || 1;
			return [-dy / len, dx / len];
		});
		const n = b.count ?? 18;
		const hues = b.hues ?? HUES;
		const strands = [];
		for (let i = 0; i < n; i++) {
			const r = rand(i + k * 97);
			strands.push({
				off: (i / Math.max(1, n - 1) - 0.5) * 2 + (r - 0.5) * 0.3,
				hue: hues[Math.floor((i / n) * hues.length) % hues.length],
				phase: r * Math.PI * 2,
				lag: r * 0.06,
				alpha: 0.5 + r * 0.5,
			});
		}
		return { ...b, pts, normals, strands, level: 1 };
	}

	class Host {
		constructor(el, build) {
			this.el = el;
			this.build = build;
			this.bundles = [];
			this.pulses = [];
			this.lit = null;
			this.visible = false;
			this.canvas = document.createElement('canvas');
			this.canvas.className = 'fibres';
			this.canvas.setAttribute('aria-hidden', 'true');
			el.prepend(this.canvas);
			this.ctx = this.canvas.getContext('2d');
			new ResizeObserver(() => this.layout()).observe(el);
			new IntersectionObserver(([e]) => {
				this.visible = e.isIntersecting;
				wake();
			}, { rootMargin: '120px 0px' }).observe(el);
			hosts.add(this);
			this.layout();
		}

		layout() {
			const r = this.el.getBoundingClientRect();
			const dpr = Math.min(devicePixelRatio || 1, 2);
			this.w = r.width;
			this.h = r.height;
			this.canvas.width = Math.round(r.width * dpr);
			this.canvas.height = Math.round(r.height * dpr);
			this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
			this.bundles = this.build(r).map(prepare);
			for (const b of this.bundles) b.level = this.lit == null || this.lit === b.id ? 1 : 0.35;
			wake();
		}

		find(id) {
			return this.bundles.find((b) => b.id === id);
		}

		light(id) {
			this.lit = id;
			wake();
		}

		fire(id, { dur = 1.4, len = 0.22, reverse = false } = {}) {
			return new Promise((done) => {
				if (reduced) return done();
				this.pulses.push({ id, start: performance.now() / 1000, dur, len, reverse, done });
				wake();
			});
		}

		draw(time) {
			const { ctx } = this;
			const isDark = dark.matches;
			ctx.clearRect(0, 0, this.w, this.h);
			ctx.globalCompositeOperation = isDark ? 'lighter' : 'source-over';
			ctx.lineCap = 'round';
			let busy = false;

			for (const b of this.bundles) {
				const goal = this.lit == null || this.lit === b.id ? 1 : 0.35;
				b.level += (goal - b.level) * 0.08;
				if (Math.abs(goal - b.level) > 0.01) busy = true;

				// Ambient pulses keep a bundle alive on its own.
				if (b.ambient && !reduced && (!b.nextAt || time >= b.nextAt)) {
					if (b.nextAt) this.pulses.push({ id: b.id, start: time, dur: b.ambient.dur ?? 2.4, len: b.ambient.len ?? 0.25, reverse: false });
					b.nextAt = time + (b.ambient.every ?? 3) * (0.7 + Math.random() * 0.6);
				}
				if (b.ambient && !reduced) busy = true;

				const base = (b.alpha ?? 0.22) * (isDark ? 0.7 : 1) * b.level;
				for (const s of b.strands) {
					ctx.beginPath();
					for (let k = 0; k <= SAMPLES; k++) {
						const [x, y] = this.at(b, s, k, time);
						k ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
					}
					ctx.strokeStyle = s.hue;
					ctx.globalAlpha = base * s.alpha;
					ctx.lineWidth = b.width ?? 1;
					ctx.stroke();
				}
			}

			// Pulses: a bright tail drawn as short sub-paths of rising opacity.
			this.pulses = this.pulses.filter((p) => {
				const b = this.find(p.id);
				const head = (time - p.start) / p.dur;
				if (!b || head > 1 + p.len + 0.08) {
					p.done?.();
					return false;
				}
				busy = true;
				for (const s of b.strands) {
					const h = head - s.lag;
					const from = Math.max(0, h - p.len), to = Math.min(1, h);
					if (to <= from) continue;
					const k0 = Math.floor(from * SAMPLES), k1 = Math.ceil(to * SAMPLES);
					for (let k = k0; k < k1; k++) {
						const t = (k - (h - p.len) * SAMPLES) / (p.len * SAMPLES);
						const a = this.at(b, s, p.reverse ? SAMPLES - k : k, time);
						const c = this.at(b, s, p.reverse ? SAMPLES - k - 1 : k + 1, time);
						ctx.beginPath();
						ctx.moveTo(a[0], a[1]);
						ctx.lineTo(c[0], c[1]);
						ctx.strokeStyle = s.hue;
						ctx.globalAlpha = Math.max(0, Math.min(1, t)) ** 2 * (isDark ? 0.9 : 0.85) * s.alpha;
						ctx.lineWidth = (b.width ?? 1) * 1.6;
						ctx.stroke();
					}
				}
				return true;
			});
			ctx.globalAlpha = 1;
			return busy || (!reduced && this.bundles.some((b) => b.sway));
		}

		at(b, s, k, time) {
			const t = k / SAMPLES;
			const env = envelope(b.shape, t);
			const sway = reduced ? 0 : Math.sin(time * 0.7 + s.phase + t * 5) * (b.sway ?? 0) * env;
			const d = (s.off * (b.spread ?? 24) / 2) * env + sway;
			const [x, y] = b.pts[k];
			const [nx, ny] = b.normals[k];
			return [x + nx * d, y + ny * d];
		}
	}

	function wake() {
		if (!frame) frame = requestAnimationFrame(render);
	}

	function render(ms) {
		frame = 0;
		const time = ms / 1000;
		let again = false;
		for (const h of hosts) if (h.visible && h.draw(time)) again = true;
		if (again && !document.hidden) wake();
	}

	document.addEventListener('visibilitychange', wake);
	dark.addEventListener('change', wake);

	window.Fibres = {
		mount: (el, build) => new Host(el, build),
		rel(el, host) {
			const a = el.getBoundingClientRect(), b = host.getBoundingClientRect();
			return { x: a.left - b.left, y: a.top - b.top, w: a.width, h: a.height };
		},
	};
})();
