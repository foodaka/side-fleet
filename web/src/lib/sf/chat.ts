// Chat apps: the assistants people already chat with (Hermes, OpenClaw, Claude
// Code) connect to the fleet over MCP at `/mcp`, with an API token. This is what
// the Chat apps page offers and the setup it hands out for each one.
import type { ApiScope } from './types';

export type ChatAppId = 'hermes' | 'openclaw' | 'claude' | 'other';

export interface ChatApp {
	id: ChatAppId;
	name: string;
	hint: string;
	/** It can follow a setup message itself (it has a terminal and edits its own config). */
	canSelfSetup: boolean;
}

export const CHAT_APPS: ChatApp[] = [
	{ id: 'hermes', name: 'Hermes', hint: 'Hermes Agent by Nous Research', canSelfSetup: true },
	{ id: 'openclaw', name: 'OpenClaw', hint: 'Your OpenClaw assistant', canSelfSetup: true },
	{ id: 'claude', name: 'Claude Code', hint: 'On your computer', canSelfSetup: true },
	{ id: 'other', name: 'Something else', hint: 'Any app with MCP, or a script', canSelfSetup: false }
];

export interface Access {
	scope: ApiScope;
	name: string;
	hint: string;
	/** How a connection with this access reads in the list. */
	short: string;
}

export const ACCESS: Access[] = [
	{
		scope: 'read',
		name: 'Check on tasks',
		hint: "See what's running, how tasks went and which files changed. It can't start or change anything.",
		short: 'Can check on tasks'
	},
	{
		scope: 'tasks',
		name: 'Run tasks',
		hint: 'Also start tasks, reply to them, stop them and open pull requests. Tasks edit files but never run commands without asking.',
		short: 'Can run tasks'
	},
	{
		scope: 'full',
		name: 'Run tasks and approve commands',
		hint: "Also say yes when a task asks to run a command, and start tasks that run commands without asking. Only for an assistant you trust with your machines.",
		short: 'Can run tasks and approve commands'
	}
];

export const accessOf = (scope: ApiScope) => ACCESS.find((a) => a.scope === scope) ?? ACCESS[0];

export const mcpUrl = () => `${location.origin}/mcp`;

/** What to add to the app so it can reach Sidefleet. */
export function setupSnippet(app: ChatAppId, token: string): string {
	const url = mcpUrl();
	switch (app) {
		case 'hermes':
			return `mcp_servers:\n  sidefleet:\n    url: "${url}"\n    headers:\n      Authorization: "Bearer ${token}"`;
		case 'openclaw':
			return `openclaw mcp set sidefleet '${JSON.stringify({ url, transport: 'streamable-http', headers: { Authorization: `Bearer ${token}` } })}'`;
		case 'claude':
			return `claude mcp add --transport http --scope user sidefleet ${url} --header "Authorization: Bearer ${token}"`;
		case 'other':
			return `URL: ${url}\nHeader: Authorization: Bearer ${token}`;
	}
}

/** Where the snippet goes, said plainly. */
export function setupSteps(app: ChatAppId): { before: string; after: string } {
	switch (app) {
		case 'hermes':
			return {
				before: 'On the computer where Hermes runs, add this to ~/.hermes/config.yaml (if the file already has an mcp_servers: line, add the sidefleet part under it):',
				after: 'Then send /reload-mcp to Hermes, or wait a minute: it picks the change up on its own.'
			};
		case 'openclaw':
			return {
				before: 'On the computer where OpenClaw runs, run this in a terminal:',
				after: 'OpenClaw picks it up with your next message.'
			};
		case 'claude':
			return {
				before: 'Run this in a terminal:',
				after: 'Then start Claude Code (or a new session). Sidefleet shows up in /mcp.'
			};
		case 'other':
			return {
				before: 'Add an MCP server to your app with this address and header (Streamable HTTP). A script can call the API at the same address with the same header.',
				after: ''
			};
	}
}

/** A message to send the assistant so it sets itself up. */
export function setupMessage(app: ChatAppId, token: string): string {
	const how: Record<Exclude<ChatAppId, 'other'>, string> = {
		hermes: 'Add this MCP server to your config in ~/.hermes/config.yaml (under mcp_servers), then reload your MCP servers:',
		openclaw: 'Run this command to add the Sidefleet MCP server:',
		claude: 'Run this command to add the Sidefleet MCP server for all my projects:'
	};
	if (app === 'other') return setupSnippet(app, token);
	return [
		'Please connect yourself to my Sidefleet, so you can run coding tasks on my machines for me.',
		how[app],
		setupSnippet(app, token),
		"When it's connected, use Sidefleet's fleet_overview tool and tell me which machines you can see."
	].join('\n\n');
}

/** What to try once it's connected. */
export const EXAMPLES = [
	"What's running on my fleet?",
	'Fix the failing test in my web repo',
	'How did that task go? Show me what changed',
	'Open a pull request for it'
];

// ---- updates in chat ----
// Sidefleet tells the app when a task finishes, fails, needs an OK or gets a pull
// request; the app turns that into a message in the chat. Claude Code has no
// chat to message, so it doesn't get these.

/** The app a connection was made for (its name is the app's). */
export function appOf(name: string): ChatAppId {
	const a = CHAT_APPS.find((a) => a.id !== 'other' && a.name.toLowerCase() === name.toLowerCase());
	return a?.id ?? 'other';
}

export const hasUpdates = (app: ChatAppId) => app !== 'claude';

export const updatesKind = (app: ChatAppId) => (app === 'hermes' ? 'hermes' : app === 'openclaw' ? 'openclaw' : 'webhook');

/** Where the app listens by default, to show as an example. */
export const updatesPlaceholder: Record<ChatAppId, string> = {
	hermes: 'https://my-server.example.com:8644',
	openclaw: 'https://my-server.example.com:18789',
	claude: '',
	other: 'https://example.com/sidefleet-webhook'
};

/** The full address updates go to: the app's own path on the address given. */
export function updatesUrl(app: ChatAppId, address: string): string {
	const a = address.trim().replace(/\/+$/, '');
	if (app === 'hermes' && !a.includes('/webhooks/')) return `${a}/webhooks/sidefleet`;
	if (app === 'openclaw' && !a.includes('/hooks/')) return `${a}/hooks/agent`;
	return a;
}

/** The address as the person typed it, from the stored URL. */
export function updatesAddress(app: ChatAppId, url: string): string {
	if (app === 'hermes') return url.replace(/\/webhooks\/sidefleet$/, '');
	if (app === 'openclaw') return url.replace(/\/hooks\/agent$/, '');
	return url;
}

/** Where Hermes delivers its messages. */
export const PLATFORMS = [
	{ id: 'telegram', name: 'Telegram' },
	{ id: 'whatsapp', name: 'WhatsApp' },
	{ id: 'discord', name: 'Discord' },
	{ id: 'slack', name: 'Slack' },
	{ id: 'signal', name: 'Signal' }
];

function hermesSubscribe(secret: string, platform: string) {
	return `hermes webhook subscribe sidefleet --prompt "{text}" --deliver ${platform} --mirror-to-session --secret ${secret} --description "Sidefleet task updates"`;
}

const openclawHooks = (secret: string) => `hooks: {\n  enabled: true,\n  token: "${secret}",\n  path: "/hooks",\n},`;

/** What to do on the app's side so updates arrive. */
export function updatesSetup(app: ChatAppId, secret: string, platform: string): { before: string; snippet: string; after: string } {
	switch (app) {
		case 'hermes':
			return {
				before: 'On the computer where Hermes runs, turn on its webhooks if they are off (add WEBHOOK_ENABLED=true to ~/.hermes/.env and restart Hermes). Then run:',
				snippet: hermesSubscribe(secret, platform),
				after: 'Then press Send a test update below.'
			};
		case 'openclaw':
			return {
				before: 'On the computer where OpenClaw runs, add this to ~/.openclaw/openclaw.json (inside the outer { }), then run openclaw gateway restart:',
				snippet: openclawHooks(secret),
				after: 'Then press Send a test update below.'
			};
		default:
			return {
				before:
					'Sidefleet POSTs JSON (event_type, text, task) to your address. To check it came from Sidefleet: X-Webhook-Signature-V2 is the hex HMAC-SHA256 of "<X-Webhook-Timestamp>.<body>" with this secret:',
				snippet: secret,
				after: ''
			};
	}
}

/** A message to send the assistant so it sets updates up itself. */
export function updatesMessage(app: ChatAppId, secret: string, platform: string): string {
	if (app === 'hermes') {
		return [
			'Please set yourself up so Sidefleet can tell you about my coding tasks, and you can tell me here.',
			'1. Make sure your webhook server is on: WEBHOOK_ENABLED=true in ~/.hermes/.env (port 8644). If you have to change it, restart your gateway.',
			`2. Run this command. If you know this chat's id, add --deliver-chat-id with it so updates come to this chat:\n\n${hermesSubscribe(secret, platform)}`,
			'When a Sidefleet update arrives, tell me about it in a short message and help me act on it with the Sidefleet tools.'
		].join('\n\n');
	}
	return [
		'Please set yourself up so Sidefleet can tell you about my coding tasks, and you can tell me here.',
		`Turn on your webhooks: in your config (~/.openclaw/openclaw.json) set\n\n${openclawHooks(secret)}\n\nthen restart your gateway (openclaw gateway restart).`,
		'When a Sidefleet update arrives, tell me about it in a short message and help me act on it with the Sidefleet tools.'
	].join('\n\n');
}
