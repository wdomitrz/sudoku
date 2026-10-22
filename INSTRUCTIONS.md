# Coding Instructions

This is the Rust PWA template. Copy it, rename the crate, and you have the next
app. The rules below are the whole contract: no `Makefile`, no ESLint, no
Prettier, no `npx`, no `node`, no bundler, no framework, no runtime install.

## Build

- Build a small static progressive web app in Rust, plain HTML, and as little JavaScript as the platform allows.
- Write all application logic, state and rendering in Rust.
- Treat `cargo build` as the whole build: it writes a publishable site into `dist/`.
- Keep the app a library, not a program: `crate-type = ["cdylib", "rlib"]`, no `[[bin]]`.
- Add no server, no `serve` subcommand and no health check; any file host can publish `dist/`.

## The two builds

- Build twice, in this order, because there are two targets: `cargo build --locked --lib --target wasm32-unknown-unknown --release`, then `wasm-bindgen --target web --no-typescript --out-dir dist --out-name app …`, then `touch build.rs && cargo build --release --locked`.
- Keep that order, and use `--out-name app` everywhere: the worker's cache version is hashed from every other file in `dist/`, the wasm included, and the artefact names must match across the family of apps.
- Keep the `touch`: `build.rs` writes into the source tree, so cargo cannot see its output changed and would skip the second build.
- Guard `build.rs` on `TARGET` and return early when it starts with `wasm`, or it fails on the step producing the files it writes.
- Write in place, each file under a scratch name and renamed over its target; never replace `dist/`, or you delete the wasm and ship a site with no app in it.
- Watch source paths in `cargo:rerun-if-changed`, not output names; cargo compares them against real files, so watching an output name means the script never re-runs.

## The shell

- Keep `src/ui.html` a static shell: structure, one inline `<style>` block, meta tags, and empty containers with ids.
- Give the page exactly one `<script type="module">`, whose whole body is `import('./app.js').then(m => m.default()).catch(...)`.
- Put no application logic in that loader — no event listener, no state, no `fetch` — and keep the catch, so a failure says plainly that the app could not start.
- Declare no manual wasm ABI: no `instantiateStreaming`, no `wasm.exports`, no hand-written pointer protocol.
- Register the service worker from Rust, not from the shell.

## The code

- Keep the app's logic in a `web-sys`-free module such as `src/<domain>.rs`, free of `web-sys`, `js-sys` and `wasm-bindgen`, so plain `cargo test` compiles and runs it.
- Keep every `document`, `localStorage` and service-worker call in `src/ui.rs`, and nothing else there.
- Compile `src/ui.rs` with `#[cfg(target_arch = "wasm32")]`, and start the app with `#[wasm_bindgen(start)]` so the loader needs no argument list and no glue.
- Hold the state in one type: a handler mutates it, drops the borrow, then calls `render`.
- Make one function own every word the app shows about the state, and render from that state; never let DOM text, classes or attributes become the source of truth.
- Clamp and validate on the way into the state; saturate rather than wrap.
- Read stored values defensively: anything missing, unparseable or out of range becomes a fresh state, and a browser API that throws is a normal condition, not a reason to fail startup.
- Capture owned `Rc` clones in an event closure, never borrowed handles, and `forget()` the closure so the listener cannot dangle.
- Name constants and functions. Do not hide important behaviour in clever one-liners.
- Keep `web-sys` features to the subset used; add `Navigator` before calling `window.navigator()`.
- Pin `wasm-bindgen = "=0.2.128"` exactly and check the CLI reports the same; a mismatch fails at load time, not at compile time.
- Add a runtime dependency only when the need is explicit. Do not add one for simple state, formatting, storage or dates.

## Files and icons

- Keep `src/<domain>.rs` for the logic, `src/ui.rs` for the DOM, `src/ui.html` for the shell.
- Keep `src/service-worker.js` a committed template for install, activate and fetch only, with exactly one `__VERSION__` for `build.rs` to substitute.
- Assemble `manifest.webmanifest` in `build.rs`, and build its icon list from the sizes actually rasterized so the two cannot drift.
- Keep `id`, `start_url` and `scope` as `"./"`, and every other URL relative, so one build mounts anywhere.
- Use `"display": "standalone"` unless the app specifically needs another mode.
- Keep `assets/icon.svg` as the committed, authoritative icon — simple, one colour, transparent — and never replace it with a PNG.
- Pick an icon colour that stays legible on a light surface and a dark one, and measure it rather than eyeballing it: a single static icon cannot clear 4.5:1 on both, so aim for the 3:1 bar for a graphical object and split the difference.
- Rasterize the 192 and 512 install PNGs from that SVG at build time; they are build output.
- Publish `assets/icon.svg` into `dist/` unchanged, beside the PNGs derived from it: the page links it and the worker precaches it.
- Publish every file the page links or the worker precaches; a 404 in `caches.addAll` rejects the whole install, so one un-published file takes the service worker with it.
- Link the SVG directly as the favicon, and the PNGs only for the manifest and `apple-touch-icon`.
- Scale the rasterizer with an `f32`, keep `tiny-skia` at `0.12` to match `resvg 0.48`, and pin the icon's digest in `tests/shell.rs` when it is the author's original.
- Delete the JavaScript template's files — `app.js`, `sw.js`, `style.css`, `index.html`, `manifest.json`, `Makefile`, `eslint.config.mjs` — when you adopt this template.

## Storage and offline

- Store data in `localStorage` only when persistence is useful, under an app-specific key prefix.
- Write nothing on first load; an app the user never touched should leave no trace.
- Cache the shell on install, drop old caches on activate, and claim clients.
- Never call `skipWaiting`: an update must not swap the wasm under a live tab.
- Serve from an allowlist of `GET` requests, and leave everything else alone.
- Report offline readiness in the page, not only in the console; it needs one successful visit over HTTPS or localhost.

## What the user sees

- Never let user-visible text name the implementation: no "Rust", "WebAssembly", "wasm", "bindings" or "compile" in markup, in any string written from Rust, or in a meta description.
- Say what the user gets or loses instead: "Simple PWA could not start. Reload the page, or check your connection."
- Allow "JavaScript" only in a `<noscript>`, where naming the thing the user must switch on is the instruction itself.
- Keep implementation vocabulary in comments and in `AGENTS.md`, where it is useful.
- Label a control for what it does, never for what it used to do, and let the visible text be the accessible name rather than overriding it.
- Give every status line something to say, and make sure it says it: a line frozen on its initial text is decoration pretending to be information.
- Describe the app, not its build, in `<meta name="description">`; it is what a search engine and a share sheet show.

## Accessibility

- Use `button` for actions, `a` for navigation, and a label for every input.
- Preserve visible focus states; never remove the outline without replacing it with something equally visible.
- Use `aria-live` or `role="status"` for dynamic text users need to notice.
- Use `disabled` to say an action is unavailable; it is what a keyboard and a screen reader both see, and it needs no `aria-label`.
- Keep touch targets large enough for mobile use, and avoid interactions that require hover.
- Do not use viewport-scaled font sizes for normal UI text, and keep text inside its containers on small screens.

## Look and scope

- Keep the author's colours, layout and wording. This is a rewrite, not a redesign.
- Use `prefers-color-scheme` when it is easy, set `color-scheme` alongside it, and respect `prefers-reduced-motion`.
- Start from the palette in `src/ui.html` rather than inventing colours: it is the same dark-first token set the server this app is served beside uses, so a PWA opened from that origin does not look like a different product.
- Give every foreground/background pair at least 4.5:1 contrast, and re-check any colour you change instead of assuming the token set already passed.
- Show the user what the app does, and only what they can act on. Do not put a heading, a logo or a status line on the page to fill it, and never announce good news they did not ask about — "ready for offline use" is a fact about the cache, not about their app. A line earns its place only by telling the reader something changed or something is wrong.
- Keep colors simple, but avoid making the whole app one undifferentiated hue.
- Keep layout responsive with simple `grid`, `flex`, `width: min(...)` and media queries.
- Keep CSS selectors purposeful: classes for styling, IDs for unique Rust hooks.
- Prefer the simplest complete app behavior, and the browser platform APIs over dependencies.
- Keep features narrow and direct; avoid modes, settings, screens, persistence, gestures or decoration unless they clearly improve the core workflow.

## Verification

- Commit a minimal `Cargo.toml` before anything else, or the workflow refuses the repository as a project directory.
- Derive a test's expectations from the source of truth, never duplicate them: a hand-kept list of file names is a second copy of the truth, and it drifts.
- Extract the service worker's `ASSETS` allowlist and assert every entry is a file the build publishes; that is the check that catches a referenced file nobody writes.
- Treat `cargo test --locked` as the minimum verification command; it needs neither the wasm target nor the generator.
- Unit-test the domain module for real — the rules, the clamps, the formatting, the edge cases — and keep the shell's invariants in `tests/shell.rs`: one script tag, no absolute URLs, one `__VERSION__`, no tracked artefact.
- Run clippy on both targets; a clean host build says nothing about the wasm one.
- Run both build steps before finishing, in order, so the site you verified is the site you shipped.
- Never write a test that skips, soft-passes or is `#[ignore]`d to get a change through.
- Keep verification notes honest, and mention any check that was not run.
- Put both build steps in `.github/workflows/build.yml`, and have the workflow verify the built site rather than trust a green job.
- Write that check's multi-line Python as a `<<'PY'` heredoc, never `python3 -c`: an indented `-c` body breaks on shell quoting, not on Python.
- Lint the wasm target with the same `RUSTFLAGS`/`rustc-cfg` the crate builds with; `web-sys` changes an accessor's return type under `--cfg=web_sys_unstable_apis`, so a leg without it is a false green.
- Add no compiler flag this app does not need: a clean wasm build needs no `--cfg=web_sys_unstable_apis`, and carrying one lints a different program from the one that ships.
- Put the GitHub Pages deploy in its own workflow, never in `build.yml`, and scope `pages: write`/`id-token: write` to the deploy job alone; a fork PR has neither permission nor the environment.
- Deploy on master only, never on tags, gate it on `github.ref == 'refs/heads/master' && !github.event.repository.fork` in that one `if:`, and hand `deploy-pages` the artifact by `artifact_name`; v5 has no `artifact_id` input and ignores one. The fork clause is not tidiness: a copy of this template is usually made into a fork, which has a `master` too, so a branch-name-only gate publishes from both and dies on "Ensure GitHub Pages has been enabled". Test `repository.fork`, never a repository variable — a variable would have to be set on the user's own repo, so the gate ships silently off on the one repo that owns the site.
- Check the built site by naming every file, not by counting: a file that is referenced but never written does not change the count.
- Fail on unexpected files as well as missing ones; a file nobody meant to publish is as much a defect as one that is absent, and neither shows up in a green build.
- Check the manifest against what was actually rasterized, not against a copy of the source.
- Keep a pinned digest in one place only, and assert it against the file; a digest copied into a second file is a second copy of the truth that drifts silently.
- Assert a workflow's settings from its own text with comments stripped, and read the value out rather than searching for a fixed string; commenting a line out otherwise satisfies the assertion.

## Never commit build artifacts

- Keep `target/` and `dist/` in `.gitignore`, with a comment saying why.
- Enforce it in `tests/shell.rs`: nothing generated tracked, and the install PNGs absent from the source tree.
- Commit only sources, plus `Cargo.lock`, which is a source input for an application.
- Never commit in the main worktree; cut a feature branch and release from it.
- Keep `README.md` short and `AGENTS.md` honest: decisions and verification commands, not a wish list.