/*
 * The page's little stories: the hero task routed to a machine, the setup steps,
 * the live fleet, the phone thread, failover, restore/fork and the screenshots.
 * Every loop only advances while its section is on screen.
 */
(() => {
	const $ = (s, r = document) => r.querySelector(s);
	const $$ = (s, r = document) => [...r.querySelectorAll(s)];
	const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
	const sleep = (ms) => new Promise((r) => setTimeout(r, reduced ? Math.min(ms, 60) : ms));
	const icon = (id) => `<svg><use href="#i-${id}" /></svg>`;
	const esc = (s) => s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);

	// Resolves as soon as el is (or next becomes) visible.
	const seen = new Map();
	const watcher = new IntersectionObserver((entries) => {
		for (const e of entries) {
			const s = seen.get(e.target);
			s.on = e.isIntersecting;
			if (s.on) s.waiters.splice(0).forEach((w) => w());
		}
	}, { threshold: 0.2 });
	function onScreen(el) {
		let s = seen.get(el);
		if (!s) {
			s = { on: false, waiters: [] };
			seen.set(el, s);
			watcher.observe(el);
		}
		return s.on ? Promise.resolve() : new Promise((r) => s.waiters.push(r));
	}

	/* ───────── Nav and reveals ───────── */
	const nav = $('.nav');
	const onScroll = () => nav.classList.toggle('scrolled', scrollY > 8);
	addEventListener('scroll', onScroll, { passive: true });
	onScroll();

	const revealer = new IntersectionObserver((entries) => {
		for (const e of entries) if (e.isIntersecting) {
			e.target.classList.add('in');
			revealer.unobserve(e.target);
		}
	}, { rootMargin: '0px 0px -8% 0px' });
	$$('.reveal').forEach((el) => revealer.observe(el));

	/* ───────── Hero ───────── */
	const hero = $('.hero');
	const send = $('#hero-send');
	const nodes = Object.fromEntries($$('[data-node]').map((n) => [n.dataset.node, n]));
	const heroFibres = Fibres.mount(hero, () => {
		const s = Fibres.rel(send, hero);
		const from = { x: s.x + s.w / 2, y: s.y + s.h / 2 };
		return Object.entries(nodes).map(([id, n], i) => {
			const r = Fibres.rel(n, hero);
			const to = { x: r.x - 2, y: r.y + r.h / 2 };
			const below = to.y > from.y + 40;
			return {
				id, from, to,
				c1: { x: from.x + 20 + i * 14, y: below ? from.y + (to.y - from.y) * 0.75 : from.y + 140 },
				c2: { x: to.x - Math.max(90, (to.x - from.x) * 0.55), y: to.y },
				count: 16, spread: 20 + i * 4, sway: 1.6, alpha: 0.2,
			};
		});
	});

	const TASKS = [
		{
			text: 'Add a password strength meter to sign-up', agent: 'claude', name: 'Claude Code', model: 'Opus 5.5', repo: 'foodaka/shop', node: 'mini', branch: 'sidefleet/7f3a',
			events: [
				['Read', 'src/routes/signup/+page.svelte'],
				['Grep', 'passwordRules'],
				['Edit', 'signup/+page.svelte', '+34', '−3'],
				['Bash', 'pnpm test signup · 12 passed'],
			],
			end: '2 files changed · pushed sidefleet/7f3a',
		},
		{
			text: 'Bump axum to 0.9 and fix the build', agent: 'codex', name: 'Codex', model: 'default', repo: 'foodaka/api', node: 'vps', branch: 'sidefleet/41af',
			events: [
				['Edit', 'Cargo.toml', '+1', '−1'],
				['Bash', 'cargo build · 3 errors'],
				['Edit', 'src/routes.rs', '+18', '−22'],
				['Bash', 'cargo test · 112 passed'],
			],
			end: '4 files changed · ready to review',
		},
		{
			text: 'Write the release notes for 0.4', agent: 'opencode', name: 'opencode', model: 'Kimi K2', repo: 'foodaka/side-fleet', node: 'mbp', branch: 'sidefleet/d07b',
			events: [
				['Bash', 'git log v0.3..HEAD --oneline'],
				['Read', 'README.md'],
				['Write', 'CHANGELOG.md', '+46', ''],
			],
			end: 'CHANGELOG.md written · 1 file changed',
		},
	];

	const feed = $('#hc-feed');
	const input = $('.hc-input');
	const typed = $('#hc-text');
	const agentPill = $('#hc-agent');
	const repoPill = $('#hc-repo');

	function say(html, cls = 'ev') {
		const d = document.createElement('div');
		d.className = cls;
		d.innerHTML = html;
		feed.append(d);
		while (feed.children.length > 9) feed.firstElementChild.remove();
		return d;
	}

	async function heroLoop() {
		await sleep(1100);
		for (let i = 0; ; i = (i + 1) % TASKS.length) {
			await onScreen(hero);
			const t = TASKS[i];
			const node = nodes[t.node];
			const label = node.querySelector('.nn').firstChild.textContent.trim();

			feed.style.transition = 'opacity .4s';
			feed.style.opacity = '0';
			await sleep(400);
			feed.replaceChildren();
			feed.style.opacity = '1';

			agentPill.innerHTML = `${icon(t.agent)}<b>${t.name}</b><em>${t.model}</em>`;
			repoPill.textContent = t.repo;
			agentPill.classList.add('flash');
			setTimeout(() => agentPill.classList.remove('flash'), 600);
			await sleep(500);

			input.classList.add('typing');
			for (const ch of t.text) {
				typed.textContent += ch;
				await sleep(28 + Math.random() * 46);
			}
			send.classList.add('ready');
			await sleep(450);
			send.classList.add('press');
			await sleep(140);
			send.classList.remove('press', 'ready');
			typed.textContent = '';
			input.classList.remove('typing');
			say(esc(t.text), 'msg');
			say(`${icon('arrow')}Routing to the least busy machine with ${t.repo} and ${t.name}…`, 'ev sys');

			heroFibres.light(t.node);
			await heroFibres.fire(t.node, { dur: 1.3 });
			node.classList.add('claimed');
			node.querySelector('.ns b').textContent = 'running';
			say(`${icon(t.node === 'vps' ? 'server' : t.node === 'mbp' ? 'laptop' : 'mini')}Claimed by ${label} · worktree <span class="k">${t.branch}</span>`, 'ev sys');
			await sleep(700);

			for (const [tool, arg, add, del] of t.events) {
				heroFibres.fire(t.node, { dur: 0.9, len: 0.14, reverse: true });
				say(`<span class="k">${tool}</span>${esc(arg)}${add ? ` <span class="add">${add}</span>` : ''}${del ? ` <span class="del">${del}</span>` : ''}`);
				await sleep(850);
			}
			say(`${icon('check')}${t.end}`, 'ev sys done');
			node.classList.remove('claimed');
			node.classList.add('done');
			node.querySelector('.ns b').textContent = 'done';
			heroFibres.light(null);
			await sleep(2600);
			node.classList.remove('done');
			node.querySelector('.ns b').textContent = 'idle';
		}
	}
	heroLoop();

	/* ───────── Steps ───────── */
	const term = $('[data-type-on-view]');
	onScreen(term).then(async () => {
		await sleep(500);
		const el = $('[data-type]', term);
		for (const ch of el.dataset.type) {
			el.textContent += ch;
			await sleep(70);
		}
		await sleep(300);
		term.classList.add('done');
	});

	const checks = $('[data-checks]');
	onScreen(checks).then(async () => {
		await sleep(700);
		for (const c of $$('.ck', checks)) {
			c.classList.add('on');
			await sleep(420);
		}
	});

	const claim = $('.claim');
	(async () => {
		for (;;) {
			await onScreen(claim);
			await sleep(900);
			claim.classList.add('go');
			await sleep(900);
			claim.classList.add('got');
			await sleep(3200);
			claim.classList.remove('go', 'got');
		}
	})();

	/* ───────── Fleet ───────── */
	const fleet = $('#fleet-panel');
	const rows = $$('.fm', fleet).map((row, i) => ({
		row,
		canvas: $('canvas', row),
		label: $('.fm-cpu b', row),
		data: Array.from({ length: 30 }, () => [38, 12, 4][i]),
		target: [38, 12, 4][i],
	}));
	function spark({ canvas, data }) {
		const ctx = canvas.getContext('2d');
		const dpr = Math.min(devicePixelRatio || 1, 2);
		if (canvas.width !== 120 * dpr) {
			canvas.width = 120 * dpr;
			canvas.height = 28 * dpr;
		}
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, 120, 28);
		const step = 120 / (data.length - 1);
		const y = (v) => 26 - (v / 100) * 24;
		ctx.beginPath();
		data.forEach((v, i) => (i ? ctx.lineTo(i * step, y(v)) : ctx.moveTo(0, y(v))));
		ctx.lineWidth = 1.5;
		ctx.strokeStyle = '#8b5cf6';
		ctx.stroke();
		ctx.lineTo(120, 28);
		ctx.lineTo(0, 28);
		const g = ctx.createLinearGradient(0, 0, 0, 28);
		g.addColorStop(0, 'rgba(139,92,246,.22)');
		g.addColorStop(1, 'rgba(139,92,246,0)');
		ctx.fillStyle = g;
		ctx.fill();
	}
	rows.forEach(spark);
	(async () => {
		let tick = 0;
		for (;;) {
			await onScreen(fleet);
			await sleep(reduced ? 4000 : 650);
			tick++;
			for (const r of rows) {
				if (Math.random() < 0.08) r.target = Math.random() < 0.5 ? 8 + Math.random() * 20 : 35 + Math.random() * 50;
				const last = r.data[r.data.length - 1];
				const next = Math.max(2, Math.min(97, last + (r.target - last) * 0.25 + (Math.random() - 0.5) * 9));
				r.data.push(next);
				r.data.shift();
				r.label.textContent = `${Math.round(next)}%`;
				spark(r);
			}
			if (tick % 7 === 0) {
				// A task lands on the least busy machine that can take it.
				const free = rows.slice(0, 2).sort((a, b) => a.data.at(-1) - b.data.at(-1))[0];
				free.row.classList.add('pulse');
				free.target = Math.min(90, free.target + 30);
				const badge = $('.badge', free.row);
				const n = (parseInt(badge.textContent) || 0) + 1;
				badge.textContent = `${n > 3 ? 1 : n} running`;
				setTimeout(() => free.row.classList.remove('pulse'), 900);
			}
		}
	})();

	/* ───────── Phone ───────── */
	const phone = $('.phone');
	const phFeed = $('#ph-feed');
	const phReply = $('#ph-reply');
	const SCRIPT = [
		['t', "I'll add a theme toggle to the settings page and wire it to the existing CSS variables."],
		['tool', 'Read', 'routes/settings/+page.svelte'],
		['tool', 'Grep', '--background'],
		['t', 'Defaulting to the system setting, with a manual override.'],
		['tool', 'Edit', 'settings/+page.svelte  +38 −2'],
		['reply', 'also remember the choice per device'],
		['tool', 'Bash', 'pnpm check · 0 errors'],
		['shot'],
		['t', 'Done. The choice is saved in localStorage, so each device keeps its own.'],
	];
	function phSay(html, cls) {
		const d = document.createElement('div');
		d.className = cls;
		d.innerHTML = html;
		phFeed.append(d);
		while (phFeed.children.length > 14) phFeed.firstElementChild.remove();
	}
	(async () => {
		for (;;) {
			phFeed.replaceChildren();
			for (const [n, [kind, a, b]] of SCRIPT.entries()) {
				await onScreen(phone);
				if (kind === 't') phSay(esc(a), 't');
				if (kind === 'tool') phSay(`<b>${a}</b><span>${esc(b)}</span>`, 'tool');
				if (kind === 'shot') phSay('<span>screenshot.png</span>', 'shot');
				if (kind === 'reply') {
					phReply.parentElement.classList.add('typing');
					phReply.textContent = '';
					for (const ch of a) {
						phReply.textContent += ch;
						await sleep(45);
					}
					await sleep(400);
					phReply.textContent = 'Reply…';
					phReply.parentElement.classList.remove('typing');
					phSay(esc(a), 'me');
					phSay('Queued · runs after this turn', 'queued');
				}
				await sleep(n < 2 ? 300 : kind === 't' ? 1500 : 1000);
			}
			await sleep(3500);
		}
	})();

	/* ───────── Tabs ───────── */
	function tabs(name, onPick) {
		const list = $(`[data-tabs="${name}"]`);
		const pick = (id, byUser) => {
			$$('[role="tab"]', list).forEach((b) => b.setAttribute('aria-selected', String(b.dataset.tab === id)));
			onPick(id, byUser);
		};
		list.addEventListener('click', (e) => {
			const b = e.target.closest('[role="tab"]');
			if (b) pick(b.dataset.tab, true);
		});
		return pick;
	}

	/* ───────── Failover ───────── */
	const fo = $('#fo-card');
	const SCENES = {
		sleep: { a: 'MacBook Pro', aNote: 'Codex', drop: 'asleep', b: 'Mac mini', bNote: 'Codex · continues', why: 'MacBook Pro stopped responding (no heartbeat for 90s)', icons: ['laptop', 'mini'] },
		limit: { a: 'Mac mini', aNote: 'Claude Code · work account', drop: 'usage limit until 14:00', b: 'vps-fra-1', bNote: 'Claude Code · personal', why: 'Mac mini hit its usage limit until 14:00', icons: ['mini', 'server'] },
	};
	let scene = 'sleep';
	let run = 0;
	const lanes = $$('.fo-lane', fo);
	const segA = $('.seg.a', fo), segB = $('.seg.b', fo), drop = $('#fo-drop'), cursor = $('#fo-cursor');
	const log = $('#fo-log');
	const DROP = 0.38, MOVE = 0.43, END = 0.94;

	function setScene(id) {
		scene = id;
		const s = SCENES[id];
		lanes[0].querySelector('b').textContent = s.a;
		lanes[0].querySelector('small').textContent = s.aNote;
		lanes[1].querySelector('b').textContent = s.b;
		lanes[1].querySelector('small').textContent = s.bNote;
		lanes[0].querySelector('.fo-who use').setAttribute('href', `#i-${s.icons[0]}`);
		lanes[1].querySelector('.fo-who use').setAttribute('href', `#i-${s.icons[1]}`);
		$('#fo-drop-label').textContent = s.drop;
		run++;
		failover(run);
	}
	tabs('fo', (id) => setScene(id));

	async function failover(me) {
		const s = SCENES[scene];
		const lines = [
			[0.12, `1m 12s  pushed sidefleet/41af 3c9e01`],
			[0.3, `3m 00s  pushed sidefleet/41af a41f2c`],
			[DROP, `3m 48s  ${s.why}`, 'w'],
			[MOVE, `4m 18s  moved to ${s.b}, from a41f2c with a digest of the thread`, 'g'],
			[0.7, `7m 00s  pushed sidefleet/41af e07d9b`],
			[END, `9m 24s  done · 7 files changed, nothing lost`, 'g'],
		];
		log.replaceChildren();
		fo.classList.remove('finished');
		drop.classList.remove('on');
		const total = reduced ? 10 : 11000;
		let elapsed = 0, last = performance.now(), shown = 0;
		while (me === run) {
			await onScreen(fo);
			await new Promise(requestAnimationFrame);
			const now = performance.now();
			elapsed += Math.min(now - last, 50);
			last = now;
			const p = reduced ? 1 : Math.min(1, elapsed / total);
			const bar = segA.parentElement.getBoundingClientRect().width;
			segA.style.width = `${Math.min(p, DROP) * 100}%`;
			segB.style.left = `${MOVE * 100}%`;
			segB.style.width = `${Math.max(0, Math.min(p, END) - MOVE) * 100}%`;
			drop.style.left = `${DROP * 100}%`;
			drop.classList.toggle('on', p >= DROP);
			cursor.style.transform = `translateX(${Math.min(p, END) * bar}px)`;
			const mins = Math.min(p, END) * 10;
			$('#fo-time').textContent = `${Math.floor(mins)}m ${String(Math.floor((mins % 1) * 60)).padStart(2, '0')}s`;
			$('#fo-hops').textContent = p >= MOVE ? '2' : '1';
			while (shown < lines.length && p >= lines[shown][0]) {
				const [, text, cls] = lines[shown++];
				const d = document.createElement('div');
				d.textContent = text;
				if (cls) d.className = cls;
				log.append(d);
				const sha = text.match(/pushed \S+ (\w+)/);
				if (sha) $('#fo-push').textContent = sha[1];
			}
			if (log.children.length > 4) log.firstElementChild.remove();
			if (p >= END) {
				fo.classList.add('finished');
				fo.querySelector('.fo-title .badge').lastChild.textContent = 'Succeeded';
				await sleep(3500);
				if (me !== run) return;
				fo.querySelector('.fo-title .badge').lastChild.textContent = 'Running';
				setScene(scene === 'sleep' ? 'limit' : 'sleep');
				$$('[data-tabs="fo"] [role="tab"]').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.tab === scene)));
				return;
			}
		}
	}
	setScene('sleep');

	/* ───────── Restore / fork ───────── */
	const thread = $('#thread');
	const msgs = $$('.tm', thread);
	msgs.forEach((m) => m.classList.toggle('you-msg', !!$('.who.you', m)));
	const banner = $('#thread-banner');
	let rfMode = 'restore', rfRun = 0;
	const pickRf = tabs('rf', (id) => {
		rfMode = id;
		$$('[data-rf-note]').forEach((p) => (p.hidden = p.dataset.rfNote !== id));
		rfLoop(++rfRun);
	});
	async function rfLoop(me) {
		for (let cycles = 0; me === rfRun; cycles++) {
			thread.classList.remove('rewound');
			banner.className = 'thread-banner';
			msgs.forEach((m) => m.classList.remove('target'));
			$$('.act', thread).forEach((a) => a.classList.remove('press'));
			await onScreen(thread);
			await sleep(900);
			if (me !== rfRun) return;
			const target = msgs[rfMode === 'restore' ? 2 : 1];
			target.classList.add('target');
			await sleep(1100);
			$('.act', target).classList.add('press');
			await sleep(350);
			if (me !== rfRun) return;
			if (rfMode === 'restore') {
				thread.classList.add('rewound');
				banner.innerHTML = `${icon('check')}Files restored to before this message · <u>Undo</u>`;
				banner.className = 'thread-banner restore on';
			} else {
				banner.innerHTML = `${icon('codex')}New thread in Codex on sidefleet/b81c, from after this message`;
				banner.className = 'thread-banner fork on';
			}
			await sleep(3800);
			if (cycles % 2 === 1 && me === rfRun) {
				pickRf(rfMode === 'restore' ? 'fork' : 'restore');
				return;
			}
		}
	}
	rfLoop(rfRun);

	/* ───────── Branches ───────── */
	const branches = $('[data-fibres="branches"]');
	Fibres.mount(branches, () => {
		const root = Fibres.rel($('[data-br-root]', branches), branches);
		const from = { x: root.x + root.w, y: root.y + root.h / 2 };
		return $$('[data-br]', branches).map((li, i) => {
			const r = Fibres.rel(li, branches);
			const stacked = r.x < root.x + root.w;
			const to = stacked ? { x: r.x + 18, y: r.y } : { x: r.x, y: r.y + r.h / 2 };
			return {
				id: i, from: stacked ? { x: root.x + 24, y: root.y + root.h } : from, to,
				c1: stacked ? { x: root.x + 24, y: to.y } : { x: from.x + (to.x - from.x) * 0.5, y: from.y },
				c2: stacked ? { x: root.x + 24, y: to.y } : { x: from.x + (to.x - from.x) * 0.5, y: to.y },
				count: 10, spread: 10, alpha: 0.3, sway: 0.6,
				hues: [['#8b5cf6', '#5c98ff'], ['#5c98ff', '#3fb8c9'], ['#f08a2e', '#ff6fae'], ['#13a05a', '#8b5cf6']][i],
				ambient: { every: 2.6 + i * 0.7, dur: 1.6 },
			};
		});
	});

	/* ───────── Footer arc ───────── */
	const arc = $('[data-fibres="arc"]');
	Fibres.mount(arc, (r) => {
		const w = r.width, h = r.height;
		return [{
			id: 'arc', from: { x: -60, y: 0 }, to: { x: w + 60, y: 0 },
			c1: { x: w * 0.18, y: h * 1.32 }, c2: { x: w * 0.82, y: h * 1.32 },
			count: 34, spread: Math.min(90, w * 0.06), shape: 'flat', alpha: 0.2, sway: 2.2,
			ambient: { every: 2.2, dur: 4.2, len: 0.16 },
		}];
	});

	/* ───────── Screenshots ───────── */
	const shots = $$('[data-shot]');
	let shotUser = false;
	const pickShot = tabs('shots', (id, byUser) => {
		if (byUser) shotUser = true;
		shots.forEach((img) => img.classList.toggle('on', img.dataset.shot === id));
	});
	(async () => {
		const ids = shots.map((s) => s.dataset.shot);
		for (let i = 0; !shotUser; ) {
			await onScreen($('.shot-frame'));
			await sleep(4500);
			if (shotUser) return;
			i = (i + 1) % ids.length;
			pickShot(ids[i]);
		}
	})();

	/* ───────── Analytics ───────── */
	// Which calls to action people use, as Vercel Web Analytics events.
	document.addEventListener('click', (e) => {
		const el = e.target.closest('a, [data-copy]');
		if (!el) return;
		const where = el.closest('header') ? 'nav' : el.closest('.hero') ? 'hero' : el.closest('.closing') ? 'closing' : el.closest('footer') ? 'footer' : 'page';
		if (el.dataset.copy) window.va('event', { name: 'Copy command', data: { command: el.dataset.copy.split(' ')[0] === 'curl' ? 'install' : el.dataset.copy.split(' ')[1] } });
		else if (el.getAttribute('href') === '#install') window.va('event', { name: 'Install click', data: { where } });
		else if (el.hostname === 'github.com') window.va('event', { name: 'GitHub click', data: { where } });
	});

	/* ───────── Early access ───────── */
	// Posts to /api/early-access, which forwards the request to Telegram.
	const ea = $('#early-access');
	if (ea) {
		const msg = $('.ea-msg', ea);
		const email = ea.elements.email;
		ea.addEventListener('submit', async (e) => {
			e.preventDefault();
			const ok = /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email.value.trim());
			email.setAttribute('aria-invalid', String(!ok));
			msg.classList.toggle('err', !ok);
			if (!ok) {
				msg.textContent = 'Enter a valid email address.';
				return email.focus();
			}
			const btn = $('button', ea);
			btn.disabled = true;
			btn.textContent = 'Sending…';
			msg.classList.remove('err');
			try {
				const r = await fetch('/api/early-access', {
					method: 'POST',
					headers: { 'content-type': 'application/json' },
					body: JSON.stringify({ email: email.value, note: ea.elements.note.value, company: ea.elements.company.value }),
				});
				const data = await r.json().catch(() => ({}));
				if (!r.ok) throw new Error(data.error || 'Something went wrong. Try again in a minute.');
				ea.classList.add('done');
				msg.textContent = "You're on the list. We'll email you when there's a spot.";
				window.va?.('event', { name: 'Early access request' });
			} catch (err) {
				msg.classList.add('err');
				msg.textContent = err.message;
				btn.disabled = false;
				btn.textContent = 'Request early access';
			}
		});
	}

	/* ───────── Copy ───────── */
	$$('[data-copy]').forEach((b) =>
		b.addEventListener('click', async () => {
			try {
				await navigator.clipboard.writeText(b.dataset.copy);
			} catch {
				return;
			}
			b.classList.add('ok');
			$('use', b).setAttribute('href', '#i-check');
			setTimeout(() => {
				b.classList.remove('ok');
				$('use', b).setAttribute('href', '#i-copy');
			}, 1400);
		}),
	);
})();
