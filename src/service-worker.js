// Browser lifecycle/cache plumbing only; all app logic remains Rust.
//
// The whole worker is install, activate, fetch. Four rules hold it together:
//
// * Every path is relative to `self.location`, so the app installs and works
//   from whatever subdirectory it is published under.
// * The cache name is content-versioned by `build.rs`, which hashes the bytes
//   of every other shell file — the wasm included — and this template. Change
//   the app, or this caching logic, and the cache name changes with it.
// * No `skipWaiting`. An update takes over once the old tabs are gone, so a
//   live app never swaps the wasm out from under itself.
// * The fetch handler only ever answers for a URL inside this app's own
//   directory, checked on every request rather than assumed from the scope.
//   Scope is a registration's claim, not a promise, and these apps share one
//   origin with pages that are not this app.

const ROOT = new URL('./', self.location.href);
const CACHE = 'sudoku-' + ROOT.pathname + '-__VERSION__';
const ASSETS = ['./', 'app.js', 'app_bg.wasm', 'manifest.webmanifest', 'icon-192.png', 'icon-512.png', 'icon.svg', 'index.html', 'style.css'].map(p => new URL(p, ROOT).href);
const IS_OWN = url => url.startsWith(ROOT.href);

self.addEventListener('install', event => {
  event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(ASSETS)));
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
  if (event.request.method !== 'GET' || !IS_OWN(url) || !ASSETS.includes(url)) return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const cached = await cache.match(event.request);
    return cached || fetch(event.request);
  })());
});