// POST /api/feedback {message, email, build, company}: forwards feedback from the Sidefleet web page
// (Settings > Send feedback) to Telegram, the same chat as early-access requests. Self-hosted
// control planes run on any origin, so this answers CORS for all of them.
// `company` is a honeypot: people never see it, so a filled one is a bot and gets a quiet 200.

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export default async function handler(req, res) {
	res.setHeader('Access-Control-Allow-Origin', '*');
	res.setHeader('Access-Control-Allow-Methods', 'POST, OPTIONS');
	res.setHeader('Access-Control-Allow-Headers', 'content-type');
	if (req.method === 'OPTIONS') return res.status(204).end();
	if (req.method !== 'POST') {
		res.setHeader('Allow', 'POST, OPTIONS');
		return res.status(405).json({ error: 'Method not allowed' });
	}
	const body = typeof req.body === 'object' && req.body ? req.body : {};
	const message = String(body.message ?? '').trim();
	const email = String(body.email ?? '').trim();
	const build = String(body.build ?? '').trim().slice(0, 120);
	if (body.company) return res.status(200).json({ ok: true });
	if (!message) return res.status(400).json({ error: 'Write something first.' });
	if (message.length > 4000) return res.status(400).json({ error: 'Keep it under 4,000 characters.' });
	if (email && (!EMAIL.test(email) || email.length > 254)) return res.status(400).json({ error: 'Enter a valid email address, or leave it empty.' });

	const { TELEGRAM_BOT_TOKEN: token, TELEGRAM_CHAT_ID: chat } = process.env;
	if (!token || !chat) return res.status(503).json({ error: 'Feedback is not set up yet. Try again soon.' });

	const where = [req.headers['x-vercel-ip-city'], req.headers['x-vercel-ip-country']].filter(Boolean).map(decodeURIComponent).join(', ');
	const about = [email && `Email: ${email}`, build && `Build: ${build}`, where && `From: ${where}`].filter(Boolean);
	const text = ['Sidefleet feedback', message, ...(about.length ? ['', ...about] : [])].join('\n');
	try {
		const r = await fetch(`https://api.telegram.org/bot${token}/sendMessage`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ chat_id: chat, text, disable_web_page_preview: true }),
		});
		if (!r.ok) throw new Error(`Telegram ${r.status}`);
	} catch (e) {
		console.error('feedback:', e.message);
		return res.status(502).json({ error: 'Something went wrong. Try again in a minute.' });
	}
	return res.status(200).json({ ok: true });
}
