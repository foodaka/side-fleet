// Notes you leave on lines of a task's diff, sent to its agent as one message in the same
// thread, and how the agent answered each when it comes back.

export interface Note {
	file: string;
	/** The line's number on the side it's on: the new file for added and unchanged lines, the old one for removed. */
	line: number;
	side: 'old' | 'new';
	/** The line as it reads, so the agent (and you, later) can find it. */
	snippet: string;
	text: string;
}

export interface Answer {
	state: 'done' | 'kept' | 'unclear';
	text: string;
}

export const noteKey = (n: Pick<Note, 'file' | 'line' | 'side'>) => `${n.file}:${n.side}:${n.line}`;
export const where = (n: Pick<Note, 'file' | 'line' | 'side'>) => `${n.file}:${n.line}${n.side === 'old' ? ' (removed line)' : ''}`;

/** The one message the agent gets. It asks for an answer per note in a shape the desk can read back. */
export function noteMessage(notes: Note[], pr: number | null): string {
	const head = pr ? `Before I merge #${pr}, notes on specific lines:` : 'Notes on specific lines of your changes:';
	const items = notes.map((n, i) => `${i + 1}. ${where(n)}\n   \`${n.snippet.trim().slice(0, 160)}\`\n   ${n.text}`);
	const ask =
		'Make the changes you agree with. Where you disagree, leave it and say why. ' +
		'End your reply with one line per note, starting with its number and "Done" or "Kept", like:\n' +
		'1. Done: reverted the timeout.\n2. Kept: the crate handles quoting, which is where hand-written exports break.';
	return [head, '', ...items, '', ask].join('\n');
}

const LINE = /^\s*(?:[-*]\s*)?\**(\d+)[.):]\**\s*\**(done|kept|declined|skipped|not done)\**\s*[:.—–-]?\s*(.*)$/i;

/** Read the agent's answer to each note from its reply; notes it didn't answer come back `unclear`. */
export function readAnswers(reply: string, count: number): Answer[] {
	const found = new Map<number, Answer>();
	for (const raw of reply.split('\n')) {
		const m = LINE.exec(raw);
		if (!m) continue;
		const n = Number(m[1]);
		if (n < 1 || n > count || found.has(n)) continue;
		const word = m[2].toLowerCase();
		found.set(n, { state: word === 'done' ? 'done' : 'kept', text: m[3].trim() });
	}
	return Array.from({ length: count }, (_, i) => found.get(i + 1) ?? { state: 'unclear', text: '' });
}
