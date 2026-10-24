// Sidefleet's service worker: only for "Notify me" (web push). It shows a
// notification when a task starts waiting on you and opens that task when the
// notification is clicked. It does not cache anything or handle fetches.
// A push holds the task's title and why it waits, never agent output.

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));

const windows = () => self.clients.matchAll({ type: 'window', includeUncontrolled: true });

self.addEventListener('push', (event) => {
	let d = {};
	try {
		d = event.data ? event.data.json() : {};
	} catch {
		/* not JSON: show the generic text */
	}
	const url = new URL(d.url || '/', self.location.origin).href;
	event.waitUntil(
		(async () => {
			// Already looking at that task: nothing to tell.
			const open = (await windows()).some((w) => w.focused && w.visibilityState === 'visible' && w.url === url);
			if (open) return;
			await self.registration.showNotification(d.title || 'A task is waiting on you', {
				body: d.label || 'Open Sidefleet to see it.',
				tag: d.id ? 'sidefleet-task-' + d.id : 'sidefleet',
				renotify: true,
				icon: '/icon-192.png',
				badge: '/icon-192.png',
				data: { url }
			});
		})()
	);
});

self.addEventListener('notificationclick', (event) => {
	event.notification.close();
	const url = (event.notification.data && event.notification.data.url) || self.location.origin + '/';
	event.waitUntil(
		(async () => {
			const w = (await windows()).find((c) => new URL(c.url).origin === self.location.origin);
			if (w) {
				await w.focus();
				w.postMessage({ type: 'open', url });
				return;
			}
			await self.clients.openWindow(url);
		})()
	);
});
