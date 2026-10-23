// POST /api/early-access {email, note, company}: forwards a hosted early-access request to
// Telegram. TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID are set on the Vercel project.
// `company` is a honeypot: people never see it, so a filled one is a bot and gets a quiet 200.

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export default async function handler(req, res) {
	if (req.method !== 'POST') {
		res.setHeader('Allow', 'POST');
		return res.status(405).json({ error: 'Method not allowed' });
	}
	const body = typeof req.body === 'object' && req.body ? req.body : {};
	const email = String(body.email ?? '').trim();
	const note = String(body.note ?? '').trim();
	if (body.company) return res.status(200).json({ ok: true });
	if (!EMAIL.test(email) || email.length > 254) return res.status(400).json({ error: 'Enter a valid email address.' });
	if (note.length > 500) return res.status(400).json({ error: 'Keep the note under 500 characters.' });

	const { TELEGRAM_BOT_TOKEN: token, TELEGRAM_CHAT_ID: chat } = process.env;
	if (!token || !chat) return res.status(503).json({ error: 'Sign-ups are not open yet. Try again soon.' });

	const where = [req.headers['x-vercel-ip-city'], req.headers['x-vercel-ip-country']].filter(Boolean).map(decodeURIComponent).join(', ');
	const text = ['Sidefleet hosted: early access request', `Email: ${email}`, note && `Note: ${note}`, where && `From: ${where}`].filter(Boolean).join('\n');
	try {
		const r = await fetch(`https://api.telegram.org/bot${token}/sendMessage`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ chat_id: chat, text, disable_web_page_preview: true }),
		});
		if (!r.ok) throw new Error(`Telegram ${r.status}`);
	} catch (e) {
		console.error('early-access:', e.message);
		return res.status(502).json({ error: 'Something went wrong. Try again in a minute.' });
	}
	return res.status(200).json({ ok: true });
}
