//! Write the site to `dist/` while the crate compiles.
//!
//! The app is a static site, so publishing it is a file copy plus three small
//! derivations. `dist/` is the whole product and the only copy: eight files,
//! from two builds.
//!
//! | `dist/` file      | written by |
//! |-------------------|------------|
//! | `app.js`          | `wasm-bindgen`, after the wasm build |
//! | `app_bg.wasm`     | `wasm-bindgen`, after the wasm build |
//! | `index.html`      | this script, from `src/ui.html`, byte for byte |
//! | `service-worker.js` | this script, `__VERSION__` substituted |
//! | `icon.svg`        | this script, from `assets/icon.svg` |
//! | `icon-192.png`    | this script, rasterized from `assets/icon.svg` |
//! | `icon-512.png`    | this script, rasterized from `assets/icon.svg` |
//! | `manifest.webmanifest` | this script, assembled from `MANIFEST` |
//! | (the eight above) | hashed into the worker's cache version |
//!
//! The SVG is published as well as rasterized, and that is not optional: the
//! page links it as the favicon and the worker precaches it, and a URL that
//! 404s does not merely fail one request — `caches.addAll` rejects the whole
//! install if any listed URL is missing, so an un-published icon leaves the app
//! with no service worker and no offline support at all, presenting as a
//! mysterious caching bug rather than as a missing file.
//!
//! **Order matters.** The cache version is derived from the bytes of every
//! other shell file, including the wasm, so this script must run *after*
//! `wasm-bindgen`. Run it first and it pins a version to whatever the previous
//! build left behind — a service worker that never invalidates, shipping an app
//! older than its own cache name. That is why AGENTS.md and the workflow both
//! run the wasm step first, and why that step ends with
//! `touch build.rs && cargo build --release`: this script writes into the
//! source tree rather than `OUT_DIR`, so cargo cannot see that its output
//! changed and would otherwise skip the second, otherwise identical build.
//!
//! Two details that have each cost somebody an hour:
//!
//! * It watches **source** paths, not output names. Cargo compares these
//!   against real files, so watching `dist/index.html` watches a file that
//!   never changes and the script then never re-runs.
//! * It writes each file under a scratch name and renames it over its target,
//!   so a host serving `dist/` never serves a half-written file. And it writes
//!   *in place* rather than replacing the directory, because the two wasm
//!   artefacts live there and are not ours to delete.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// The install identity of the app, carried into `manifest.webmanifest`.
///
/// These are the original template's own values and they are the template's
/// identity: change them here, not in a JSON file. The icon list is appended
/// from what was actually rasterized, so it cannot drift from the PNGs.
///
/// The colours are the app's own, taken from the palette in `src/ui.html`.
///
/// `background_color` is dark on purpose. It only ever colours the splash
/// screen, which is shown for a moment and cannot be scheme-aware, so it is the
/// one surface where a fixed value costs nothing and a light one flashes.
///
/// `theme_color` is deliberately **absent**. Chrome for Android prefers a
/// manifest `theme_color` over the per-scheme `<meta name="theme-color"
/// media=...>` tags the shell declares, and a manifest cannot express a scheme
/// variant — so putting one here would pin the installed app's status bar dark
/// even on a device in light mode, and silently override both meta tags.
/// Omitting it lets the media-scoped tags win, which is the only way the status
/// bar can follow the system.
const MANIFEST: Manifest = Manifest {
    name: "Sudoku",
    short_name: "Sudoku",
    background_color: "#0e1116",
};

/// The app-shell files this script copies verbatim, and where each comes from.
///
/// The two wasm artefacts are deliberately absent: they are written by
/// `wasm-bindgen` into `dist/`, they are never committed, and they have no
/// committed source to copy from.
const SHELL: &[(&str, &str)] = &[
    ("index.html", "src/ui.html"),
    ("style.css", "src/style.css"),
];

/// The committed SVG every icon is derived from.
const ICON: &str = "assets/icon.svg";

/// The install icon sizes, in pixels. A square PNG at each, for the manifest
/// and for `apple-touch-icon`.
const ICON_SIZES: [u32; 2] = [192, 512];

/// The icon's published name in `dist/`. The same bytes as `ICON`; see `main`.
const ICON_OUT: &str = "icon.svg";

/// A web app manifest, minus the parts this script derives.
struct Manifest {
    name: &'static str,
    short_name: &'static str,
    background_color: &'static str,
}

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());

    // The site is built for the host target only. This script also runs during
    // `cargo build --lib --target wasm32-unknown-unknown`, and at that moment
    // `dist/app.js` and `dist/app_bg.wasm` are the *output* of that build, so
    // writing the site there would fail on the very step that produces it.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm") {
        return;
    }

    // Watch the sources, not the destinations (see the module comment).
    for (_, source) in SHELL {
        println!("cargo:rerun-if-changed={source}");
    }
    println!("cargo:rerun-if-changed=src/service-worker.js");
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=build.rs");

    let mut built: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, source) in SHELL {
        let bytes = std::fs::read(root.join(source))
            .unwrap_or_else(|error| panic!("reading {source}: {error}"));
        built.push(((*name).to_owned(), bytes));
    }

    // The icon is rasterized at every size the manifest declares, and the
    // manifest is assembled from that same list: `assets/icon.svg` is the icon,
    // the PNGs are what it looks like at 192 and 512, and both are build output.
    let svg =
        std::fs::read(root.join(ICON)).unwrap_or_else(|error| panic!("reading {ICON}: {error}"));
    // Publish the SVG itself, not only what it rasterizes to: the page links it
    // as the favicon and the worker precaches it, and `caches.addAll` rejects
    // the entire install when any listed URL 404s.
    built.push((ICON_OUT.to_owned(), svg.clone()));
    let mut icons = Vec::new();
    for size in ICON_SIZES {
        let name = format!("icon-{size}.png");
        icons.push(icon_entry(&name, size));
        built.push((name, rasterize(&svg, size)));
    }
    built.push((
        "manifest.webmanifest".to_owned(),
        manifest(&icons).into_bytes(),
    ));

    // The worker's own template is hashed too, and is deliberately kept out of
    // `built` so it is hashed exactly once, in template form. A change to the
    // caching logic must invalidate the cache: clients holding the old worker
    // would otherwise run stale logic against new assets.
    let template = std::fs::read_to_string(root.join("src/service-worker.js"))
        .unwrap_or_else(|error| panic!("reading src/service-worker.js: {error}"));
    let version = cache_version(&built, &template);
    let worker = template.replace("__VERSION__", &version);
    assert!(
        !worker.contains("__VERSION__"),
        "the service worker still carries the version placeholder"
    );
    built.push(("service-worker.js".to_owned(), worker.into_bytes()));

    write_tree(&root.join("dist"), &built);
}

/// Render the committed SVG to a square PNG of `size` pixels.
///
/// `assets/icon.svg` is the icon. It is never replaced by a PNG and never
/// regenerated: this is the only place a raster icon comes from, so editing the
/// SVG changes every size at once.
fn rasterize(svg: &[u8], size: u32) -> Vec<u8> {
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg, &options)
        .unwrap_or_else(|error| panic!("parsing {ICON}: {error}"));
    assert!(
        tree.size().width() > 0.0 && tree.size().height() > 0.0,
        "{ICON} has no drawable area"
    );

    // 512 is a power of two, so the pixmap request can only fail if the
    // allocation fails; `Pixmap::new` returns an `Option`, not a `Result`.
    let Some(mut pixmap) = tiny_skia::Pixmap::new(size, size) else {
        panic!("a {size}x{size} pixmap could not be allocated");
    };
    // `size` is a `u32` and the transform is `f32`; every icon size is far
    // below f32's 24-bit exact-integer range, so the conversion is exact.
    // The division stays in `f32`: dividing in `f64` and casting afterwards
    // is the mistake to avoid.
    #[allow(clippy::cast_precision_loss)]
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().expect("encoding the icon as PNG")
}

/// One `manifest.webmanifest` icon entry.
fn icon_entry(name: &str, size: u32) -> serde_json::Value {
    serde_json::json!({
        "src": name,
        "sizes": format!("{size}x{size}"),
        "type": "image/png",
        // `maskable` so the icon survives an Android launcher's circle and
        // squircle crops; the glyph is centred on a transparent field for it.
        "purpose": "any maskable",
    })
}

/// The manifest, assembled from the declared identity and the icons that were
/// actually rasterized.
///
/// `id`, `start_url` and `scope` are all `"./"`, so one build works from any
/// subdirectory and installs under the path it is served from.
fn manifest(icons: &[serde_json::Value]) -> String {
    serde_json::json!({
        "id": "./",
        "name": MANIFEST.name,
        "short_name": MANIFEST.short_name,
        "start_url": "./",
        "scope": "./",
        "display": "standalone",
        "background_color": MANIFEST.background_color,
        "icons": icons,
    })
    .to_string()
}

/// A cache name derived from the bytes of every shell file except the worker,
/// and from the worker's own source.
fn cache_version(built: &[(String, Vec<u8>)], worker_template: &str) -> String {
    let mut hasher = DefaultHasher::new();
    for (name, bytes) in built {
        name.hash(&mut hasher);
        bytes.hash(&mut hasher);
    }
    worker_template.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Write every file this script owns into `dir`, in place.
///
/// In place, not by replacing the directory: the wasm step owns two files in
/// there, and a wholesale swap deleted them — leaving a publishable-looking
/// site with no app in it, and no error. Scratch-then-rename so a host never
/// observes a partial file.
fn write_tree(dir: &Path, built: &[(String, Vec<u8>)]) {
    std::fs::create_dir_all(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));

    for (name, bytes) in built {
        let path = dir.join(name);
        let scratch = dir.join(format!(".{name}.new"));
        std::fs::write(&scratch, bytes)
            .unwrap_or_else(|error| panic!("writing {}: {error}", scratch.display()));
        std::fs::rename(&scratch, &path)
            .unwrap_or_else(|error| panic!("publishing {}: {error}", path.display()));
    }
}
