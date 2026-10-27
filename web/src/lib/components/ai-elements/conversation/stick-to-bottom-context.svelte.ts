import { watch } from "runed";
import { setContext, getContext } from "svelte";

const STICK_TO_BOTTOM_CONTEXT_KEY = Symbol("stick-to-bottom-context");

/** Closer than this to the bottom, the jump-to-latest button stays hidden. */
const NEAR_BOTTOM = 80;
/** Back within this of the bottom, the view follows new content again. */
const AT_BOTTOM = 4;

/**
 * Keeps a scroller pinned to its bottom while content streams in, until the reader moves away.
 *
 * Only the reader lets go, by moving up: a wheel or key up, or a finger or the scrollbar dragging
 * the view. Content re-rendering can move scrollTop too, so a scroll alone doesn't count. A finger
 * on the screen holds the view still, so streamed text never yanks a phone back down
 * mid-gesture. Scrolling back to the very bottom, or the jump button, picks up following again.
 */
class StickToBottomContext {
	#element: HTMLElement | null = $state(null);
	#isAtBottom = $state(true);
	#stuck = true;
	/** A finger, or the mouse on the scrollbar, is moving the view. */
	#holding = false;
	#lastScrollTop = 0;
	#frame = 0;
	#resizeObserver: ResizeObserver | null = null;
	#mutationObserver: MutationObserver | null = null;

	isAtBottom = $derived(this.#isAtBottom);

	constructor() {
		watch(
			() => this.#element,
			() => {
				if (this.#element) {
					this.#setup(this.#element);
					return () => this.#cleanup();
				}
			}
		);
	}

	setElement(element: HTMLElement) {
		this.#element = element;
	}

	scrollToBottom = (behavior: ScrollBehavior = "smooth") => {
		const el = this.#element;
		if (!el) return;
		this.#stuck = true;
		el.scrollTo({ top: el.scrollHeight, behavior });
	};

	#measure() {
		const el = this.#element;
		if (!el) return 0;
		const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
		this.#isAtBottom = distance <= NEAR_BOTTOM;
		return distance;
	}

	/** Content or size changed: follow it to the bottom once per frame, unless the reader let go. */
	#schedule = () => {
		if (this.#frame) return;
		this.#frame = requestAnimationFrame(() => {
			this.#frame = 0;
			const el = this.#element;
			if (!el) return;
			if (this.#stuck && !this.#holding) el.scrollTop = el.scrollHeight;
			this.#measure();
		});
	};

	#onScroll = () => {
		const el = this.#element;
		if (!el) return;
		const distance = this.#measure();
		if (this.#holding && el.scrollTop < this.#lastScrollTop - 1) this.#stuck = false;
		else if (distance <= AT_BOTTOM) this.#stuck = true;
		this.#lastScrollTop = el.scrollTop;
	};

	// Intent arrives before the scroll it causes; let go at once so a frame of new content
	// landing in between can't pull the view back down.
	#onWheel = (e: WheelEvent) => {
		if (e.deltaY < 0) this.#stuck = false;
	};
	#onKey = (e: KeyboardEvent) => {
		if (["ArrowUp", "PageUp", "Home"].includes(e.key)) this.#stuck = false;
	};
	#onTouchStart = () => {
		this.#holding = true;
	};
	#onTouchEnd = (e: TouchEvent) => {
		if (e.touches.length) return;
		this.#release();
	};
	// A press on the scroller itself, not its content, is on its scrollbar.
	#onPointerDown = (e: PointerEvent) => {
		if (e.pointerType !== "mouse" || e.target !== this.#element) return;
		this.#holding = true;
		window.addEventListener("pointerup", this.#release, { once: true });
	};
	#release = () => {
		this.#holding = false;
		this.#schedule();
	};

	#setup(el: HTMLElement) {
		this.#lastScrollTop = el.scrollTop;
		el.addEventListener("scroll", this.#onScroll, { passive: true });
		el.addEventListener("wheel", this.#onWheel, { passive: true });
		el.addEventListener("keydown", this.#onKey);
		el.addEventListener("touchstart", this.#onTouchStart, { passive: true });
		el.addEventListener("touchend", this.#onTouchEnd, { passive: true });
		el.addEventListener("touchcancel", this.#onTouchEnd, { passive: true });
		el.addEventListener("pointerdown", this.#onPointerDown);

		// The scroller resizing (a phone keyboard opening) and the content in it growing without
		// DOM changes (fonts, highlighting, images).
		this.#resizeObserver = new ResizeObserver(this.#schedule);
		this.#resizeObserver.observe(el);
		for (const child of el.children) this.#resizeObserver.observe(child);

		this.#mutationObserver = new MutationObserver(this.#schedule);
		this.#mutationObserver.observe(el, { childList: true, subtree: true, characterData: true });

		this.#measure();
	}

	#cleanup() {
		const el = this.#element;
		this.#resizeObserver?.disconnect();
		this.#mutationObserver?.disconnect();
		cancelAnimationFrame(this.#frame);
		this.#frame = 0;
		if (el) {
			el.removeEventListener("scroll", this.#onScroll);
			el.removeEventListener("wheel", this.#onWheel);
			el.removeEventListener("keydown", this.#onKey);
			el.removeEventListener("touchstart", this.#onTouchStart);
			el.removeEventListener("touchend", this.#onTouchEnd);
			el.removeEventListener("touchcancel", this.#onTouchEnd);
			el.removeEventListener("pointerdown", this.#onPointerDown);
		}
		window.removeEventListener("pointerup", this.#release);
		this.#holding = false;
		this.#resizeObserver = null;
		this.#mutationObserver = null;
	}
}

export function setStickToBottomContext(): StickToBottomContext {
	const context = new StickToBottomContext();
	setContext(STICK_TO_BOTTOM_CONTEXT_KEY, context);
	return context;
}

export function getStickToBottomContext(): StickToBottomContext {
	const context = getContext<StickToBottomContext>(STICK_TO_BOTTOM_CONTEXT_KEY);
	if (!context) {
		throw new Error("StickToBottomContext must be used within a Conversation component");
	}
	return context;
}

export { StickToBottomContext };
