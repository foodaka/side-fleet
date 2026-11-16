// Images and files attached to a task or reply: dropped anywhere on the page,
// pasted into the box, or picked with the paperclip. Sent base64 with the message.
// Images (PNG, JPEG, GIF, WebP) go to the agent as images; anything else (markdown,
// text, code, PDFs) as a file it reads.
import { toast } from 'svelte-sonner';

/** Sent as images (raster only: an SVG could run script, so it goes as a file). */
export const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
export const MAX_IMAGES = 8;
export const MAX_FILES = 8;
const MAX_BYTES = 10 * 1024 * 1024;
/** Everything a message carries together, well under the request limit once base64. */
const MAX_TOTAL_BYTES = 40 * 1024 * 1024;

export interface Attached {
	key: string;
	file: File;
	kind: 'image' | 'file';
	/** Object URL for an image's thumbnail; empty for a file. */
	url: string;
}

interface Upload {
	media_type: string;
	data: string;
}

/** The request body's `images` and `files`. */
export interface AttachedPayload {
	images: Upload[];
	files: (Upload & { name: string })[];
}

/** `12 KB`, `3.4 MB`. */
export function fileSize(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
	return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function read(file: File): Promise<string> {
	return new Promise((resolve, reject) => {
		const r = new FileReader();
		r.onload = () => resolve(String(r.result).slice(String(r.result).indexOf(',') + 1));
		r.onerror = () => reject(new Error(`Could not read ${file.name || 'a file'}`));
		r.readAsDataURL(file);
	});
}

export class Attachments {
	items = $state<Attached[]>([]);
	/** A file is being dragged over the page. */
	dragging = $state(false);
	#depth = 0;
	/** Thumbnail keys. Not crypto.randomUUID: that only exists on https or localhost, and the
	 * control plane is often reached over plain http (a Tailscale address). */
	#next = 0;

	add(files: Iterable<File>) {
		const skipped: string[] = [];
		for (const file of files) {
			const kind = IMAGE_TYPES.includes(file.type) ? 'image' : 'file';
			const count = this.items.filter((a) => a.kind === kind).length;
			if (file.size > MAX_BYTES) skipped.push(`${file.name || 'That file'} is over 10 MB`);
			else if (kind === 'image' && count >= MAX_IMAGES) skipped.push(`At most ${MAX_IMAGES} images per message`);
			else if (kind === 'file' && count >= MAX_FILES) skipped.push(`At most ${MAX_FILES} files per message`);
			else if (this.items.reduce((n, a) => n + a.file.size, file.size) > MAX_TOTAL_BYTES) skipped.push('The attachments add up to more than 40 MB');
			else this.items.push({ key: String(this.#next++), file, kind, url: kind === 'image' ? URL.createObjectURL(file) : '' });
		}
		if (skipped.length) toast.error(skipped.length === 1 ? 'Could not attach it' : `Could not attach ${skipped.length} files`, { description: [...new Set(skipped)].join('. ') });
	}

	remove(key: string) {
		const a = this.items.find((x) => x.key === key);
		if (a?.url) URL.revokeObjectURL(a.url);
		this.items = this.items.filter((x) => x.key !== key);
	}

	clear() {
		for (const a of this.items) if (a.url) URL.revokeObjectURL(a.url);
		this.items = [];
	}

	/** The request body's `images` and `files`. */
	async payload(): Promise<AttachedPayload> {
		const images = this.items.filter((a) => a.kind === 'image');
		const files = this.items.filter((a) => a.kind === 'file');
		return {
			images: await Promise.all(images.map(async (a) => ({ media_type: a.file.type, data: await read(a.file) }))),
			files: await Promise.all(files.map(async (a) => ({ name: a.file.name || 'file', media_type: a.file.type, data: await read(a.file) })))
		};
	}

	// Window-level drag handlers: a file dropped anywhere attaches instead of the browser opening it.
	#hasFiles = (e: DragEvent) => !!e.dataTransfer && [...e.dataTransfer.types].includes('Files');
	dragenter = (e: DragEvent) => {
		if (!this.#hasFiles(e)) return;
		this.#depth++;
		this.dragging = true;
	};
	dragleave = (e: DragEvent) => {
		if (!this.#hasFiles(e)) return;
		if (--this.#depth <= 0) { this.#depth = 0; this.dragging = false; }
	};
	dragover = (e: DragEvent) => {
		if (!this.#hasFiles(e)) return;
		e.preventDefault();
		e.dataTransfer!.dropEffect = 'copy';
	};
	drop = (e: DragEvent) => {
		if (!this.#hasFiles(e)) return;
		e.preventDefault();
		this.#depth = 0;
		this.dragging = false;
		this.add(e.dataTransfer!.files);
	};
	/** Images or files pasted into the text box (a screenshot on the clipboard, a copied file). */
	paste = (e: ClipboardEvent) => {
		const files = [...(e.clipboardData?.files ?? [])];
		if (!files.length) return;
		e.preventDefault();
		this.add(files);
	};
}
