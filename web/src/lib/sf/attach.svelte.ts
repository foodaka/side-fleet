// Images attached to a task or reply: dropped anywhere on the page, pasted
// into the box, or picked with the paperclip. Sent base64 with the message.
import { toast } from 'svelte-sonner';

/** What the control plane accepts (raster only: an SVG could run script). */
export const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
export const MAX_IMAGES = 8;
const MAX_BYTES = 10 * 1024 * 1024;
/** All of a message's images together, well under the request limit once base64. */
const MAX_TOTAL_BYTES = 40 * 1024 * 1024;

export interface Attached {
	key: string;
	file: File;
	/** Object URL for the thumbnail. */
	url: string;
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
			if (!IMAGE_TYPES.includes(file.type)) skipped.push(`${file.name || 'That file'} is not a PNG, JPEG, GIF or WebP image`);
			else if (file.size > MAX_BYTES) skipped.push(`${file.name || 'That image'} is over 10 MB`);
			else if (this.items.length >= MAX_IMAGES) skipped.push(`At most ${MAX_IMAGES} images per message`);
			else if (this.items.reduce((n, a) => n + a.file.size, file.size) > MAX_TOTAL_BYTES) skipped.push('The images add up to more than 40 MB');
			else this.items.push({ key: String(this.#next++), file, url: URL.createObjectURL(file) });
		}
		if (skipped.length) toast.error(skipped.length === 1 ? 'Could not attach it' : `Could not attach ${skipped.length} files`, { description: [...new Set(skipped)].join('. ') });
	}

	remove(key: string) {
		const a = this.items.find((x) => x.key === key);
		if (a) URL.revokeObjectURL(a.url);
		this.items = this.items.filter((x) => x.key !== key);
	}

	clear() {
		for (const a of this.items) URL.revokeObjectURL(a.url);
		this.items = [];
	}

	/** The request body's `images`. */
	payload(): Promise<{ media_type: string; data: string }[]> {
		return Promise.all(
			this.items.map(
				(a) =>
					new Promise<{ media_type: string; data: string }>((resolve, reject) => {
						const r = new FileReader();
						r.onload = () => resolve({ media_type: a.file.type, data: String(r.result).slice(String(r.result).indexOf(',') + 1) });
						r.onerror = () => reject(new Error(`Could not read ${a.file.name}`));
						r.readAsDataURL(a.file);
					})
			)
		);
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
	/** Images pasted into the text box (a screenshot on the clipboard). */
	paste = (e: ClipboardEvent) => {
		const files = [...(e.clipboardData?.files ?? [])];
		if (!files.length) return;
		e.preventDefault();
		this.add(files);
	};
}
