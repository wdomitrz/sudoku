// Browser lifecycle/cache plumbing only; all app logic remains Rust.
//
// The whole worker is install, activate, fetch. Four rules hold it together:
//
// * Every path is relative to `self.location`, so the app installs and works
//   from whatever subdirectory it is published under.
// * The cache name is content-versioned by `build.rs`, which hashes the bytes
//   of every other shell file — the wasm included — and this template. Change
//   the app, or this caching logic, and the cache name changes with it.
// * `skipWaiting` exists, but only when a page asks for it by name. Never on
//   install, because that swaps the wasm out from under a live game. The page
//   asks from Rust once it has decided it can reload — see `keep_offline` in
//   `src/ui.rs`. Without this listener a new version installs and then waits
//   until every tab closes, which on a phone can be days: the app keeps working
//   and never updates, and nothing about the page suggests why.
// * The manifest is never answered from the cache. It is fetched by the browser
//   manager, not by the page, and a stale copy pins an installed app to an old
//   name, icon and start URL long after the site has moved on.
// * The fetch handler only ever answers for a URL inside this app's own
//   directory, checked on every request rather than assumed from the scope.
//   Scope is a registration's claim, not a promise, and these apps share one
//   origin with pages that are not this app.

const ROOT = new URL('./', self.location.href);
const CACHE = 'sudoku-' + ROOT.pathname + '-__VERSION__';
const ASSETS = ['./', 'app.js', 'app_bg.wasm', 'manifest.webmanifest', 'icon-192.png', 'icon-512.png', 'icon.svg', 'index.html', 'style.css'].map(p => new URL(p, ROOT).href);
const IS_OWN = url => url.startsWith(ROOT.href);
const MANIFEST = new URL('manifest.webmanifest', ROOT).href;

self.addEventListener('install', event => {
  event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(ASSETS)));
});

// The page's half of the handover: it posts this once it is ready to reload.
// Anything else a window happens to send is ignored, so a page cannot make the
// worker take over for a reason the app has not considered.
self.addEventListener('message', event => {
  if (event.data === 'SKIP_WAITING') self.skipWaiting();
});

self.addEventListener('activate', event => {
  event.waitUntil((async () => {
    const prefix = 'sudoku-' + ROOT.pathname + '-';
    for (const key of await caches.keys()) {
      if (key.startsWith(prefix) && key !== CACHE) await caches.delete(key);
    }
    await self.clients.claim();
  })());
});

self.addEventListener('fetch', event => {
  // An allowlist, not a catch-all: this worker has no business touching anything
  // it did not precache, including the pages the user navigates away to. The
  // directory check is the second half of the same rule.
  const url = event.request.url;
  // Read the manifest from the network every time, and let a failure be a
  // failure: the browser falls back to the last one it saw.
  if (url === MANIFEST) return;
  if (event.request.method !== 'GET' || !IS_OWN(url) || !ASSETS.includes(url)) return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const cached = await cache.match(event.request);
    return cached || fetch(event.request);
  })());
});