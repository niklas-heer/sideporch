// Sideporch's service worker. It shows push notifications, keeps the app
// icon's unread count, opens the right conversation when a notification is
// tapped, and shows a friendly page when there is no connection. Pages are
// never cached: a chat is only useful with the latest messages.
"use strict";

const OFFLINE_CACHE = "sideporch-offline-v1";
const OFFLINE_URL = "/offline";

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(OFFLINE_CACHE).then((cache) => cache.add(OFFLINE_URL)));
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) => Promise.all(names.filter((name) => name !== OFFLINE_CACHE).map((name) => caches.delete(name))))
      .then(() => self.clients.claim()),
  );
});

// Pages load from the network; only when that fails does the offline page
// stand in. Everything else goes straight to the network.
self.addEventListener("fetch", (event) => {
  if (event.request.mode !== "navigate") return;
  event.respondWith(fetch(event.request).catch(() => caches.match(OFFLINE_URL)));
});

function setBadge(count) {
  if (!("setAppBadge" in self.navigator) || typeof count !== "number") return Promise.resolve();
  return (count > 0 ? self.navigator.setAppBadge(count) : self.navigator.clearAppBadge()).catch(() => {});
}

self.addEventListener("push", (event) => {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch {
    data = { body: event.data?.text() ?? "" };
  }
  event.waitUntil(
    Promise.all([
      self.registration.showNotification(data.title || "Sideporch", {
        body: data.body || "",
        tag: data.tag,
        // A newer message in the same conversation replaces the older
        // notification but still alerts.
        renotify: Boolean(data.tag),
        timestamp: data.timestamp,
        icon: "/assets/icons/icon-192.png",
        badge: "/assets/icons/badge-96.png",
        data: { url: data.url || "/" },
      }),
      setBadge(data.badge),
    ]),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = new URL(event.notification.data?.url || "/", self.location.origin).href;
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then(async (windows) => {
      const exact = windows.find((client) => client.url === url);
      if (exact) return exact.focus();
      // Reuse an open Sideporch window, so the installed app doesn't stack up.
      const open = windows.find((client) => "navigate" in client);
      if (open) {
        const focused = await open.focus();
        return focused.navigate(url);
      }
      return self.clients.openWindow(url);
    }),
  );
});
