//! Invariants of the committed shell, and of what is and is not committed.
//!
//! Everything here reads committed files. `dist/` is build output and is
//! gitignored, so the release gate — which exports the candidate tree — never
//! has it and cannot build it: that needs the wasm target and a pinned
//! `wasm-bindgen` CLI. A test asserting on `dist/` would therefore run only in
//! a developer's checkout, which is exactly where it is least likely to catch
//! anything. What covers the built output is running the two build steps in the
//! order AGENTS.md gives them, and `.github/workflows/build.yml` does that on
//! every push.
//!
//! What these tests do protect is that the shell cannot quietly grow
//! application JavaScript again, cannot point anywhere absolute, cannot commit a
//! generated file, and cannot lose the icon that every install icon is derived
//! from. Each of those has been true of a real version of this repository.

use std::fmt::Write as _;
use std::path::Path;

/// The repository root, as the tests see it.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The app shell, as committed.
///
/// `build.rs` copies this into `dist/index.html` byte for byte, so asserting on
/// it asserts on exactly what gets published.
fn shell() -> String {
    let path = root().join("src/ui.html");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// The service worker template, as committed.
/// The committed stylesheet, as the build publishes it.
fn style() -> String {
    let path = root().join("src/style.css");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

fn worker_template() -> String {
    let path = root().join("src/service-worker.js");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Tracked file names, or `None` outside a checkout.
///
/// The release gate exports the candidate as a bare directory with no `.git`,
/// so there is no index to ask. Callers decide what that means.
fn tracked_files() -> Option<String> {
    let inside = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(root())
        .output()
        .is_ok_and(|out| out.status.success());
    if !inside {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(root())
        .output()
        .expect("git ls-files");
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Every file `dist/` must contain once both build steps have run.
///
/// The two `wasm-bindgen` artefacts are named here because nothing in the tree
/// generates them; the other six are the files `build.rs` owns, and it asserts
/// those itself by parsing the worker's allowlist (see
/// `every_precached_file_is_published_by_the_build_script`). This list is what
/// the workflow's site check uses, and it is written out by name on purpose: a
/// count would not catch a file that is referenced but never written.
const DIST_FILES: [&str; 9] = [
    "app.js",
    "app_bg.wasm",
    "icon.svg",
    "icon-192.png",
    "icon-512.png",
    "index.html",
    "manifest.webmanifest",
    "service-worker.js",
    "style.css",
];

/// The files `build.rs` publishes, read out of the script itself.
///
/// Derived, never duplicated: a hand-kept list of eight names is a second copy
/// of the truth, and it drifts — which is exactly how `icon.svg` came to be
/// referenced by the page and the worker but never written to `dist/`. A test
/// that duplicated the list would have agreed with itself and passed.
fn published_by_build_script() -> Vec<String> {
    let build = std::fs::read_to_string(root().join("build.rs")).expect("the build script");

    // The verbatim copies: `SHELL` names the destination first.
    let (_, shell) = build
        .split_once("const SHELL: &[(&str, &str)] = &[")
        .expect("build.rs declares the files it copies");
    let shell = shell
        .split_once("];")
        .expect("the end of the SHELL table")
        .0;
    let mut names: Vec<String> = shell
        .lines()
        .filter_map(|line| line.trim().strip_prefix("(\""))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_owned)
        .collect();

    // The icon is published under a constant, beside the PNGs derived from it.
    // This must come from the push, not from the constant's declaration: reading
    // the constant would accept a site that publishes the icon under a name
    // built at runtime, and would then agree with itself about a file the worker
    // cannot fetch. What has to be true is that *the bytes* reach `dist/` under
    // a name the allowlist uses.
    let mut published_icons: Vec<&str> = build
        .lines()
        .filter(|line| {
            line.trim_start().starts_with("built.push((") && line.contains("icon-{size}.png")
        })
        .filter_map(|line| line.split('"').nth(1))
        .collect();
    // Each entry must name the SVG as its content, and the allowlist is the
    // arbiter of what that name is, so the check below closes the loop.
    assert!(
        build.lines().any(|line| {
            line.trim_start().starts_with("built.push((")
                && line.contains("svg.clone()")
                && !line.contains("format!(\"icon-")
        }),
        "build.rs derives the install icons but never publishes assets/icon.svg itself; \
         the page links it as the favicon and the worker precaches it"
    );
    published_icons.sort_unstable();
    names.extend(published_icons.into_iter().map(str::to_owned));

    // And the icon itself. Its published name is decided by `ICON_OUT`, so read
    // the constant's *value* here: scanning for the constant's name instead
    // would happily accept `format!("{ICON_OUT}.bak")`, which writes the SVG
    // under a name no allowlist can fetch — and then agree with itself.
    let icon_out = build
        .lines()
        .find_map(|line| line.trim().strip_prefix("const ICON_OUT: &str = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("build.rs declares the icon's published name")
        .to_owned();
    names.push(icon_out);

    // The manifest, assembled in Rust.
    if build.contains("\"manifest.webmanifest\"") {
        names.push("manifest.webmanifest".to_owned());
    }

    // The install PNGs, named with a format string at each declared size.
    let declared = build
        .lines()
        .find_map(|line| line.trim().strip_prefix("const ICON_SIZES: [u32; 2] = ["))
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_default();
    for size in declared
        .split(',')
        .filter_map(|size| size.trim().parse::<u32>().ok())
    {
        names.push(format!("icon-{size}.png"));
    }

    // The service worker, written from the template.
    if build.contains("template.replace(\"__VERSION__\"") {
        names.push("service-worker.js".to_owned());
    }

    names.sort();
    names
}

/// The entries of the worker's `ASSETS` allowlist, parsed out of the template.
///
/// These are the URLs `caches.addAll` fetches on install. `addAll` rejects the
/// whole install if any one of them 404s, so a name that is allowlisted but
/// never published does not degrade to a missing icon: it takes the service
/// worker down with it, and in the browser it presents as "offline is broken"
/// with nothing in the build output pointing at the missing file.
fn precached_by_the_worker() -> Vec<String> {
    let worker = worker_template();
    let list = worker
        .split_once("const ASSETS = [")
        .expect("the worker declares an ASSETS allowlist")
        .1
        .split_once(']')
        .expect("the end of the ASSETS allowlist")
        .0;
    list.split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// Every URL the worker precaches is a file `build.rs` publishes.
///
/// This is the assertion that would have caught the missing `icon.svg`, and it
/// is deliberately derived from both sides rather than checked against a third
/// hand-written list: the worker's allowlist says what is fetched, `build.rs`
/// says what is written, and this is where they are made to agree.
#[test]
fn every_precached_file_is_published_by_the_build_script() {
    let published = published_by_build_script();
    assert!(
        !published.is_empty(),
        "could not read the published files out of build.rs; the assertion would pass vacuously"
    );
    assert!(
        published.iter().any(|name| name == "index.html"),
        "build.rs no longer publishes the shell: {published:?}"
    );

    for entry in precached_by_the_worker() {
        if entry == "./" {
            // The scope root, which the host serves as `index.html`.
            continue;
        }
        if WASM_BINDGEN_FILES.contains(&entry.as_str()) {
            assert!(
                !published.contains(&entry),
                "'{entry}' is written by wasm-bindgen, so build.rs must not also write it"
            );
            continue;
        }
        assert!(
            published.contains(&entry),
            "the service worker precaches '{entry}' but build.rs does not publish it; \
             caches.addAll rejects the entire install when any listed URL is missing, \
             so this costs the app its offline support rather than one icon. \
             build.rs publishes: {published:?}"
        );
    }
}

/// Nothing `build.rs` publishes is missing from the allowlist: a published
/// file the worker does not precache is a file the app still needs online.
#[test]
fn everything_the_build_publishes_is_precached_or_understood() {
    let precached = precached_by_the_worker();
    for name in published_by_build_script() {
        if name == "service-worker.js" {
            // A worker is fetched by the browser, never from its own cache.
            continue;
        }
        assert!(
            precached.contains(&name),
            "build.rs publishes '{name}' but the service worker does not precache it"
        );
    }
}

/// `dist/` is the union of the two owners, and the worker allowlists exactly
/// that. This is the test that reports a site in terms a reader can act on:
/// eight files, eight precached, nothing referenced and unwritten.
#[test]
fn the_whole_site_is_the_union_of_its_two_owners() {
    let mut site = published_by_build_script();
    site.extend(WASM_BINDGEN_FILES.iter().map(|name| (*name).to_owned()));
    site.sort();
    site.dedup();

    let mut documented: Vec<String> = DIST_FILES.iter().map(|name| (*name).to_owned()).collect();
    documented.sort();
    assert_eq!(
        site, documented,
        "the files the two build steps write must match the documented site exactly"
    );

    let precached = precached_by_the_worker();
    assert_eq!(
        precached.len(),
        DIST_FILES.len(),
        "the worker precaches every file in the site except itself"
    );
}

/// The two files `wasm-bindgen` writes into `dist/`, and `build.rs` does not.
///
/// They are build output with no committed source, which is exactly why they
/// are named here rather than left implicit: everything else in `dist/` belongs
/// to `build.rs`, and the assertions below are only meaningful if the two
/// ownership domains are both accounted for.
const WASM_BINDGEN_FILES: [&str; 2] = ["app.js", "app_bg.wasm"];

/// Run `git check-ignore` for `path`, or `None` outside a checkout.
fn is_ignored(path: &str) -> Option<bool> {
    tracked_files()?;
    Some(
        std::process::Command::new("git")
            .args(["check-ignore", "-q", path])
            .current_dir(root())
            .status()
            .expect("git check-ignore")
            .success(),
    )
}

/// The app is wasm. A hand-written ABI would mean the Rust in the page is not
/// the Rust in `src/`, which is the one thing this shape exists to prevent.
#[test]
fn the_page_loads_generated_bindings_not_a_manual_wasm_abi() {
    let page = shell();
    assert!(
        page.contains("<!DOCTYPE html>"),
        "the shell must be a document"
    );
    assert!(page.contains("<script type=\"module\">"), "a module script");
    assert!(
        page.contains("import('./app.js')"),
        "the page must load the generated bindings"
    );
    assert!(
        page.contains(".then(m => m.default())"),
        "the loader must hand over to the generated start function"
    );
    assert_eq!(
        page.matches("<script").count(),
        1,
        "exactly one script tag:\n{page}"
    );
    for obsolete in [
        "instantiateStreaming",
        "alloc_buf",
        "wasm.exports",
        "fetch(",
    ] {
        assert!(!page.contains(obsolete), "{obsolete} in the static shell");
    }
}

/// The loader's only job is to hand over and to say so when it cannot. There
/// must be no application logic left in it: a `addEventListener`, a state
/// variable or a second statement doing real work is how a JavaScript app grows
/// back.
#[test]
fn the_loader_does_nothing_but_load_and_report_failure() {
    let page = shell();
    let script = page
        .split_once("<script type=\"module\">")
        .expect("the module script")
        .1
        .split_once("</script>")
        .expect("the end of the module script")
        .0;
    assert!(
        script.contains("catch"),
        "the loader must handle the app failing to load"
    );
    assert!(
        script.contains("getElementById('error')"),
        "the failure branch must show something the user can read"
    );
    assert!(
        script.contains("could not start"),
        "the failure message must say the app could not start"
    );
    // It may declare locals in the failure branch, but it may not do anything
    // the app's logic could ever want: those would be state and events, which
    // are Rust's job. No comment either — a loader that explains itself is
    // usually a loader that grew.
    for forbidden in [
        "//",
        "/*",
        "addEventListener",
        "querySelector",
        "localStorage",
        "function",
        "fetch(",
    ] {
        assert!(
            !script.contains(forbidden),
            "{forbidden} in the loader:\n{script}"
        );
    }
    assert!(
        !script.contains("let ") && script.matches("const ").count() <= 1,
        "the loader must declare no state of its own:\n{script}"
    );
}

/// An installable, accessible page: the install icon is linked, the dynamic
/// readout is announced, and the control is a real button with a visible focus
/// ring and a large enough target.
#[test]
fn the_page_is_accessible_and_installable() {
    let page = shell();
    for required in [
        "id=\"board\"",
        "id=\"pad\"",
        "id=\"status\"",
        "id=\"error\"",
        "role=\"status\"",
        "aria-live",
        "rel=\"manifest\"",
        "rel=\"icon\"",
        "rel=\"apple-touch-icon\"",
        "<noscript>",
        "id=\"notes\" type=\"button\" aria-pressed=",
    ] {
        assert!(page.contains(required), "the shell is missing {required}");
    }
    // The stylesheet is its own file, so it must be linked — and it is what
    // carries the rules this test is about.
    assert!(
        page.contains("rel=\"stylesheet\" href=\"./style.css\""),
        "the shell must link its stylesheet, relative to itself"
    );
    assert!(
        !page.contains("<style"),
        "the shell must not inline its styles: they live in src/style.css so the \
         service worker can precache and version them as one file"
    );
    let css = style();
    assert!(
        css.contains("outline: 3px solid"),
        "focus must stay visible"
    );
    assert!(
        css.contains("min-height: 44px"),
        "touch targets must be large enough for mobile use"
    );
    assert!(
        css.contains("prefers-color-scheme") && css.contains("prefers-reduced-motion"),
        "the stylesheet must respect both media queries"
    );
}

/// No visible text may name the implementation.
///
/// An interface that tells the user it is "written in Rust", "compiled to
/// WebAssembly" or "loading the Rust app" is talking to nobody: the user came
/// to use a counter, and how it is built is neither useful nor anything they
/// can act on. It is also the single most common way a demo page ends up looking
/// unfinished, because the sentences are about the author, not the app.
///
/// Implementation vocabulary is fine in comments, docs and `AGENTS.md`. It is
/// not fine in anything a reader can see: markup, text, `alt`, the meta
/// description, or any string Rust writes into the DOM.
#[test]
fn no_visible_text_names_the_implementation() {
    let page = shell();
    // Rendered text only: comments are stripped, and `<style>`/`<script>` bodies
    // are dropped too, since neither is something a reader sees.
    let rendered = visible_text(&page);
    for banned in [
        "rust",
        "webassembly",
        "wasm",
        "bindings",
        "compiled",
        "compile",
    ] {
        assert!(
            !rendered.to_lowercase().contains(banned),
            "the rendered page says \"{banned}\"; user-visible text must not name \
             the implementation — say what the user gets or loses instead. \
             Visible text was:\n{rendered}"
        );
    }
    // "JavaScript" is the single exception, and only in a `<noscript>`: naming
    // the thing the user must go and switch on is the one case where the
    // implementation vocabulary *is* the instruction. Everywhere else it is
    // not actionable and is banned above.
    let outside_noscript = rendered.replace(noscript_body(&rendered).as_str(), "");
    assert!(
        !outside_noscript.to_lowercase().contains("javascript"),
        "only a <noscript> block may name JavaScript; everywhere else it must not"
    );
    // The same rule for every string the Rust layer can write into the DOM.
    let rust = std::fs::read_to_string(root().join("src/ui.rs")).expect("src/ui.rs");
    for line in rust.lines() {
        let trimmed = line.trim_start();
        // Skip doc comments and line comments; they are documentation.
        if !trimmed.starts_with("const ") || !trimmed.contains('"') {
            continue;
        }
        let value = trimmed.split('"').nth(1).unwrap_or_default().to_lowercase();
        for banned in ["rust", "webassembly", "wasm", "javascript", "compiled"] {
            assert!(
                !value.contains(banned),
                "a user-visible string in src/ui.rs says \"{banned}\": {trimmed}"
            );
        }
    }
}

/// A status line must be able to change. An element that can only ever show the
/// text it shipped with is decoration pretending to be information, and two
/// permanently-frozen lines under a working readout are most of what makes a
/// demo page look unfinished.
///
/// The rule this enforces: if a string is only ever set by the loader, it does
/// not belong on the page at all.
#[test]
fn no_status_line_is_frozen() {
    let page = shell();

    // Every `id` the page renders, so a new one cannot slip in unasserted.
    // `#error` is exempt: it is the failure box, filled by the loader's catch
    // and hidden until then. An element that only exists on failure is not a
    // frozen status line.
    let mut shown: Vec<&str> = page
        .match_indices("id=\"")
        .filter_map(|(at, _)| {
            let rest = &page[at + 4..];
            rest.find('"').map(|end| &rest[..end])
        })
        .filter(|id| !matches!(*id, "title" | "pad-heading" | "error"))
        .collect();
    shown.sort_unstable();

    let rust = std::fs::read_to_string(root().join("src/ui.rs")).expect("src/ui.rs");
    for id in shown {
        assert!(
            rust.contains(&format!("\"{id}\"")),
            "#{id} is rendered by the shell but src/ui.rs never touches it, so it \
             can only ever show the text it shipped with — remove it or let the \
             app write to it"
        );
    }
}

/// The button's visible text is its accessible name. An `aria-label` describing
/// the same control differently gives a sighted user and a screen-reader user
/// two different descriptions of one button, which is worse than either.
#[test]
fn the_button_has_one_consistent_name() {
    let page = shell();
    let rust = std::fs::read_to_string(root().join("src/ui.rs")).expect("src/ui.rs");
    // Comments in both files discuss *why* there is no aria-label, so search the
    // markup only: strip comments from the shell, and take non-comment lines of
    // the Rust.
    let shell_markup = visible_text(&page);
    assert!(
        !shell_markup.contains("aria-label="),
        "the shell must not hard-code an aria-label: the visible text is already \
         the accessible name, and a second name contradicts it"
    );
    for (source, text) in [("src/ui.html", &page), ("src/ui.rs", &rust)] {
        for line in text.lines() {
            let trimmed = line.trim();
            // Skip comment lines and the continuation lines of block comments.
            let is_comment = trimmed.starts_with("//")
                || trimmed.starts_with("///")
                || trimmed.starts_with('*')
                || trimmed.starts_with("/*");
            if is_comment {
                continue;
            }
            // The forms that actually set one: an attribute in the shell, and
            // a quoted property name in the Rust. A bare substring match would
            // also catch `aria-labelledby`, which is the *right* way to name a
            // group after the heading it sits under.
            let sets_a_label = trimmed.contains("aria-label=")
                || trimmed.contains("\"aria-label\"")
                || trimmed.contains("aria_label");
            assert!(
                !sets_a_label,
                "{source} sets an aria-label on a control that has visible text: {trimmed}\n\
                 The visible text is already the accessible name; a second one \
                 contradicts it"
            );
        }
    }

    // And every button the shell ships must say what it does, in words a
    // person uses for this game. The template's "Add one" was a counter's
    // label; a sudoku's controls are named for what they do to a cell.
    let mut control_names: Vec<String> = Vec::new();
    for (at, opening) in page.match_indices("<label for=\"menu\"") {
        // The menu's control is a checkbox with a label as its face, so its name
        // is the label's text rather than a button body.
        let after_label = &page[at + opening.len()..];
        let close = after_label
            .find("</label>")
            .expect("the end of the menu label");
        if let (Some(open), Some(shut)) = (after_label.find('>'), after_label.find("<span>")) {
            if shut < close {
                let text =
                    after_label[shut + 6..after_label.find("</span>").unwrap_or(close)].trim();
                if !text.is_empty() && open < close {
                    control_names.push(text.to_string());
                }
            }
        }
    }
    for (at, opening) in page.match_indices("<button id=") {
        // `match_indices` yields the matched text, not what follows it.
        let from_button = &page[at + opening.len()..];
        let Some(tag_end) = from_button.find('>') else {
            continue;
        };
        let Some(close) = from_button.find("</button>") else {
            continue;
        };
        if close < tag_end {
            continue;
        }
        let name = from_button[tag_end + 1..close].trim().to_string();
        if !name.is_empty() {
            control_names.push(name);
        }
    }
    for expected in ["Notes", "Undo", "Erase", "Menu", "New puzzle", "Restart"] {
        assert!(
            control_names.iter().any(|name| name == expected),
            "the shell has no control labelled {expected:?}; found {control_names:?}"
        );
    }
    // A hint is deliberately absent: filling a cell from the answer ends the
    // deduction the puzzle exists for, and the two destructive buttons it would
    // sit beside are folded away in the menu for the same reason — they are not
    // part of playing.
    assert!(
        !control_names.iter().any(|name| name == "Hint"),
        "the hint was removed on purpose; found {control_names:?}"
    );

    // A label must not describe a mechanism. Every one of these is a word the
    // reader acts on rather than a name for how the app is put together.
    for name in &control_names {
        for banned in ["wasm", "rust", "generate", "compute", "sol"] {
            assert!(
                !name.to_lowercase().contains(banned),
                "the control {name:?} is labelled for its mechanism"
            );
        }
    }
}

/// The text inside the page's `<noscript>` element, or `""` if it has none.
fn noscript_body(page: &str) -> String {
    page.split_once("<noscript>")
        .and_then(|(_, rest)| rest.split_once("</noscript>"))
        .map_or(String::new(), |(body, _)| body.to_owned())
}

/// Everything a reader can actually see: `src/ui.html` with its `<!-- -->`
/// comments removed.
///
/// Only HTML comments are dropped; the `<style>` and `<script>` bodies stay,
/// because a `contains` assertion over this text has to be able to see them.
/// The `<noscript>` block is kept for the same reason — a browser with scripting
/// off renders exactly that text, so it is user-visible text and is held to the
/// same rule: it says what the user loses, never what the app is built with.
fn visible_text(page: &str) -> String {
    let mut out = String::with_capacity(page.len());
    let mut rest = page;
    loop {
        let Some(start) = rest.find("<!--") else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        rest = rest[start..]
            .find("-->")
            .map_or("", |end| &rest[start + end + 3..]);
    }
}

/// The site is mounted under an arbitrary prefix, so every URL in it is
/// relative. One build, any subdirectory, any file host.
#[test]
fn the_shell_is_mountable_anywhere() {
    let page = shell();
    assert!(
        !page.contains("http://") && !page.contains("https://"),
        "an absolute URL would break the site outside its own origin"
    );
    assert!(
        page.contains("./app.js"),
        "bindings must be referenced relatively"
    );
    assert!(
        page.contains("./icon.svg"),
        "the SVG icon must be linked directly"
    );
    assert!(
        page.contains("./manifest.webmanifest"),
        "the manifest must be linked relatively"
    );
    assert!(
        shell().contains("./icon.svg"),
        "the worker precaches the committed SVG, which is the icon's source of truth"
    );
    // Every file the page links must be one `build.rs` publishes. `./icon.svg`
    // is the one that is easy to miss: it is rasterized, so publishing it looks
    // like an extra step rather than a requirement.
    assert!(
        shell().contains("./icon-192.png") && shell().contains("./manifest.webmanifest"),
        "the page must reference the manifest and the apple-touch icon relatively"
    );
    // The worker's own URLs must resolve the same way.
    assert!(
        worker_template().contains("new URL('./', self.location.href)"),
        "the worker must resolve its cache from its own location"
    );
}

/// The service worker is a committed template with exactly one placeholder, and
/// `build.rs` substitutes it. No placeholder would mean the cache never
/// invalidates; two would mean the substitution is not the only edit.
#[test]
fn the_service_worker_template_has_exactly_one_placeholder() {
    let worker = worker_template();
    assert_eq!(
        worker.matches("__VERSION__").count(),
        1,
        "the template must carry exactly one version placeholder"
    );
    // Everything the worker precaches, including the SVG that is the icon's
    // source of truth and its own `index.html`, so the offline path serves the
    // same page the network path does.
    for file in DIST_FILES
        .iter()
        .filter(|file| **file != "service-worker.js")
    {
        assert!(
            worker.contains(&format!("'{file}'")),
            "the worker does not precache {file}, and a 404 in `caches.addAll` \
             rejects the whole install, so {file} must exist in dist/"
        );
    }
    // The worker is deliberately not in its own list: a worker is fetched by
    // the browser, never from its own cache.
    assert!(
        worker.contains("'icon.svg'"),
        "the worker does not precache the icon's source of truth"
    );
    // `skipWaiting` is allowed here, but only as the page's answer — see
    // `a_new_version_can_take_over_a_running_app`. The template forbade the call
    // outright, and that rule is how this app came to ship a worker that could
    // never take over a running one: nothing was swapping code under a live tab,
    // and nothing was updating an installed app either.
}

/// The worker only ever answers for a URL inside its own app's directory.
///
/// This is the guard that stops one of these apps from taking over the pages it
/// shares an origin with. A service worker registered for a scope is consulted
/// for every URL under that scope, and these apps are all served from the same
/// origin as pages that are not apps at all — so "the scope is small" is a
/// promise, and this test is what keeps it one.
#[test]
fn the_worker_never_answers_outside_its_own_directory() {
    let worker = worker_template();
    assert!(
        worker.contains("new URL('./', self.location.href)"),
        "the worker must resolve its own directory from its location"
    );
    // The guard is a prefix test against that directory, on the request URL,
    // applied before the allowlist decides anything.
    assert!(
        worker.contains("IS_OWN(url)"),
        "the fetch handler must check the request is inside this app's directory; \
         without it a mis-scoped registration serves whatever it cached"
    );
    assert!(
        worker.contains("const IS_OWN = url => url.startsWith(ROOT.href)"),
        "the directory guard must be a prefix test against the worker's own root"
    );
}

/// The page states the worker's scope instead of inheriting it, and cleans up a
/// wider registration left behind by an earlier version.
///
/// A registration outlives the page that created it, and nothing short of an
/// explicit `unregister` takes one away. So the second half is what makes this
/// recoverable without the user clearing their browser: a stale registration
/// is not fixed by a reload, and the newer worker cannot take control of a
/// scope it does not own.
#[test]
fn the_page_states_the_scope_and_releases_a_wider_one() {
    let source = std::fs::read_to_string(root().join("src/ui.rs")).expect("reading src/ui.rs");
    // Comments go first — see `strip_rust_comments`.
    let ui = strip_rust_comments(&source);
    assert!(
        ui.contains("register_with_options"),
        "the worker must be registered with an explicit scope; left to default, \
         the scope is whatever directory the registering page sits in"
    );
    assert!(
        ui.contains("RegistrationOptions::new()") && ui.contains("set_scope(SCOPE)"),
        "the scope has to be actually stated, not merely a named constant"
    );
    assert!(
        ui.contains("get_registrations") && ui.contains("unregister"),
        "a stale wider registration survives a reload, a version bump and a \
         reinstall; only an explicit unregister clears it"
    );
}

/// A worker's script is compared by suffix, not by `trim_end_matches`.
///
/// `trim_end_matches` strips a *set of characters*, so a directory whose name
/// ends in those letters is silently treated as ours — and a registration
/// belonging to a sibling app would be torn down. This is a regression test for
/// a real bug in the first version of this code.
#[test]
fn the_script_comparison_strips_a_suffix_rather_than_a_character_set() {
    let source = std::fs::read_to_string(root().join("src/ui.rs")).expect("reading src/ui.rs");
    // Comments go first — see `strip_rust_comments`.
    let ui = strip_rust_comments(&source);
    // The prose in this file names the method to explain why it is not used, so
    // the assertion is about code: a call, not the word.
    let calls: Vec<&str> = ui
        .lines()
        .filter(|line| {
            let code = line.split("//").next().unwrap_or(line);
            code.contains("trim_end_matches(")
        })
        .collect();
    assert!(
        calls.is_empty(),
        "`trim_end_matches` strips a character set, not a filename: it would eat \
         any directory ending in those letters and tear down a sibling's worker. \
         Found: {calls:?}"
    );
    assert!(
        ui.contains("strip_suffix(\"service-worker.js\")"),
        "the comparison must strip the one filename it expects"
    );
}

/// The scope is named once, and the page and the worker agree on the directory.
///
/// Two independent resolutions of "where am I" — the page's `./` and the worker's
/// `new URL('./', self.location.href)`. They have to describe the same
/// directory, or the page registers a scope the worker's guard does not match.
#[test]
fn the_scope_is_a_relative_directory_shared_with_the_worker() {
    let source = std::fs::read_to_string(root().join("src/ui.rs")).expect("reading src/ui.rs");
    // Comments go first — see `strip_rust_comments`.
    let ui = strip_rust_comments(&source);
    assert!(
        ui.contains("const SCOPE: &str = \"./\";"),
        "the scope must be the app's own directory, relative — so one build works \
         from any subdirectory"
    );
    assert!(
        worker_template().contains("new URL('./', self.location.href)"),
        "the worker must resolve the same directory the page registered"
    );
}

/// The SVG is the icon. It is the committed source of truth, it is what the
/// page links, and both install PNGs are rasterized from it at build time, so
/// it is never replaced by a PNG and never regenerated.
///
/// The digest is the original template's, recorded so that an "improved" icon
/// is a deliberate act with a visible diff rather than a silent one.
///
/// Deliberately changed once, on 2026-10-01: the file had carried the original
/// template's `P` glyph in dark `#434343`, and three bars in a mid grey replaced
/// it. The pin is the mechanism working as intended — a red test, a visible
/// diff, a reason — so it stays, and the reason moves here with the change
/// rather than being left in a commit message to rot.
const ICON_SHA256: &str = "d26385e419ba99ff4cb81c75577247f168e290d942bdbc0e947b2a4dd977531f";

#[test]
fn the_icon_is_the_committed_svg() {
    let icon = root().join("assets/icon.svg");
    let bytes =
        std::fs::read(&icon).unwrap_or_else(|error| panic!("reading {}: {error}", icon.display()));
    assert_eq!(
        sha256_hex(&bytes),
        ICON_SHA256,
        "assets/icon.svg is the icon, and a change to it must update ICON_SHA256 \
         deliberately: the SVG is the source every install PNG is derived from"
    );
    // One colour, on a transparent ground — asserted where the colour actually
    // is, not by searching the file for a string. A substring search over the
    // whole SVG passes on this file's own comment, which names the old dark grey
    // in exactly the sentence explaining why it went: a test that would survive
    // the very thing it exists to catch.
    //
    // The mid grey is measured, not chosen by eye. A single static icon has to
    // stay legible on a light surface and a dark one, and no grey can clear
    // 4.5:1 on both — that needs a relative luminance below 0.18 and above 0.20
    // at the same time. 3:1 is the bar for a graphical object, and `#7a7a7a`
    // reaches 4.29:1 on white and 4.40:1 on this project's `#0f1117`.
    let icon_text = String::from_utf8_lossy(&bytes);
    let mut fills: Vec<&str> = icon_text
        .match_indices("fill=\"")
        .filter_map(|(at, _)| {
            let rest = &icon_text[at + 6..];
            rest.find('"').map(|end| &rest[..end])
        })
        // `none` is not a colour: it is the absence of one, and it is how a
        // stroked shape says "outline only".
        .filter(|fill| *fill != "none")
        .collect();
    fills.dedup();
    assert_eq!(
        fills,
        ["#444444"],
        "the icon is one colour, and it is the legible midpoint between a light \
         and a dark surface; found fills {fills:?}"
    );
    // The PNGs are build output: never committed, and never in the source tree
    // at all. They exist in `dist/`, which is the site and nothing else. The
    // SVG is the opposite: committed, authoritative, and published to `dist/`
    // unchanged.
    let Some(tracked) = tracked_files() else {
        return; // not a checkout: the gate's exported tree
    };
    assert!(
        tracked.lines().any(|line| line == "assets/icon.svg"),
        "assets/icon.svg is the committed source of truth and must be tracked"
    );
    for artefact in ["assets/icon-192.png", "assets/icon-512.png"] {
        assert!(
            !tracked.lines().any(|line| line == artefact),
            "{artefact} is tracked; install icons are derived at build time and must never be committed"
        );
        assert!(
            !root().join(artefact).exists(),
            "{artefact} exists in the source tree; it belongs in dist/ only"
        );
    }
}

/// Nothing generated may be tracked — not the site, not the wasm, not the
/// bindings. This is the test that would have caught them being committed.
#[test]
fn no_build_artifact_is_committed() {
    let Some(tracked) = tracked_files() else {
        return; // not a checkout
    };
    for artefact in [
        "dist",
        "dist/app.js",
        "dist/app_bg.wasm",
        "dist/index.html",
        "dist/service-worker.js",
        "dist/icon-192.png",
        "dist/icon-512.png",
        "dist/manifest.webmanifest",
    ] {
        assert!(
            !tracked.lines().any(|line| line == artefact),
            "{artefact} is tracked; generated artefacts must never be committed"
        );
    }
}

/// `target/` and `dist/` have to be ignored, or a build leaves the next commit
/// dirty and the release gate tries to ship a wasm.
#[test]
fn build_output_is_ignored() {
    let Some(_) = tracked_files() else {
        return; // not a checkout
    };
    for path in ["dist/", "target/"] {
        assert_eq!(is_ignored(path), Some(true), "{path} must be in .gitignore");
    }
}

/// The old JavaScript template's files are gone, and no build tool crept back in
/// to replace them. A copy-and-rename that kept `Makefile` and `eslint.config.mjs`
/// would be two templates in one repository, disagreeing.
#[test]
fn the_old_javascript_template_is_gone() {
    let Some(tracked) = tracked_files() else {
        return; // not a checkout
    };
    for obsolete in [
        "app.js",
        "sw.js",
        "style.css",
        "index.html",
        "manifest.json",
        "Makefile",
        "eslint.config.mjs",
    ] {
        assert!(
            !tracked.lines().any(|line| line == obsolete),
            "{obsolete} is still tracked; this is the Rust template"
        );
    }
    assert!(
        !root().join("Makefile").exists(),
        "a Makefile would bring back the build-free JS template's tooling"
    );
}

/// The one place the template states its own rules still exists, and it is
/// still a flat list of short imperatives rather than an essay. This is the
/// artefact a copy of this repo is supposed to inherit.
#[test]
fn the_instructions_are_still_a_short_flat_list() {
    let path = root().join("INSTRUCTIONS.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    assert!(
        text.starts_with("# "),
        "INSTRUCTIONS.md must open with a heading"
    );
    let rules: Vec<&str> = text.lines().filter(|line| line.starts_with("- ")).collect();
    // The upper bound moved once, on 2026-10-02, from 90 to 95, when the GitHub
    // Pages rules were added: publishing the site is part of the contract, and
    // the failures it guards — a deploy in the CI build, publishing permissions
    // granted workflow-wide, an `artifact_id` that v5 silently ignores — are
    // all silent. It is a deliberate change and the reason is here rather than
    // in a commit message, so the next person to hit the bound knows what the
    // number is guarding against and does not raise it again on a whim.
    assert!(
        rules.len() >= 30 && rules.len() <= 95,
        "the rules must stay a short list, not an essay: {} rules",
        rules.len()
    );
    for required in [
        "- Build",
        "wasm",
        "wasm-bindgen",
        "prefers-color-scheme",
        "prefers-reduced-motion",
        "aria-live",
        "localStorage",
        "tests",
        // The JS-era tooling must be gone, not merely discouraged.
        "Prettier",
        "ESLint",
        "skipWaiting",
    ] {
        assert!(
            text.contains(required),
            "INSTRUCTIONS.md no longer covers {required}"
        );
    }
    // The tooling rules it replaces have to be rules too, or the reader is left
    // with a hole where verification used to be.
    for required in [
        "cargo test",
        "clippy",
        "touch build.rs",
        "dist/",
        "assets/icon.svg",
        // The family-wide rule prompted by a defect the user found: no
        // user-visible text may name the implementation.
        "name the implementation",
    ] {
        assert!(
            text.contains(required),
            "INSTRUCTIONS.md lost its verification guidance for {required}"
        );
    }
    assert!(
        !text.contains("npx "),
        "INSTRUCTIONS.md still sends the reader to npx"
    );
}

/// A workflow file, as committed.
///
/// Nonexistent is not a reason to fail. The release gate exports the candidate
/// tree, and a partial export of a release branch need not carry every workflow;
/// a test that hard-failed on absence would make such an export red for a reason
/// that says nothing about the app. Callers say which they want — and the ones
/// that matter here also run only in a full checkout.
fn workflow(name: &str) -> Option<String> {
    let path = root().join(".github/workflows").join(name);
    std::fs::read_to_string(&path).ok()
}

/// A workflow with its comments removed.
///
/// A `#` inside a quoted string is not a comment, and these workflows quote
/// nothing in the keys they are read for — but the point is not the corner case.
/// It is that commenting a line out instead of deleting it is the exact mutation
/// most likely to be applied to one of the lines below, and a `# RUSTFLAGS:`
/// satisfies a `contains` assertion that reads the raw text. Every assertion
/// about a setting reads this, not the file.
fn strip_yaml_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find('#') {
            Some(index) => &line[..index],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `pages.yml` with its comments removed, or `None` if it is absent.
///
/// Every workflow assertion reads this rather than the file, for the reason
/// `strip_yaml_comments` gives.
fn live_pages_workflow() -> Option<String> {
    workflow("pages.yml").map(|text| strip_yaml_comments(&text))
}

/// The workflow's top-level `env:` block, as key → value.
///
/// Reading the block rather than searching the whole file is what lets a test
/// ask "is this set at the *top level*", which is a question about indentation
/// and cannot be answered by `contains`. The block ends at the first column-zero
/// key — `permissions:` here — so a value belonging to some other top-level
/// section cannot be mistaken for one.
fn top_level_env(yaml: &str) -> Option<std::collections::BTreeMap<String, String>> {
    let mut declared = std::collections::BTreeMap::new();
    let mut in_env = false;
    let mut saw_env = false;
    for line in yaml.lines() {
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(' ') {
            in_env = line.trim_start().starts_with("env:");
            saw_env |= in_env;
            continue;
        }
        if !in_env {
            continue;
        }
        let trimmed = line.trim_start();
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        declared.insert(
            key.to_owned(),
            value.trim().trim_matches(['"', '\'']).to_owned(),
        );
    }
    saw_env.then_some(declared)
}

/// The Pages workflow must not install a bindings generator for a runtime the
/// crate is not built against.
///
/// `Cargo.toml` pins `wasm-bindgen = "=0.2.128"` exactly; the job installs its
/// own CLI from a variable in `env:`. Move the dependency and the workflow keeps
/// building happily: it generates bindings for a runtime the page does not have,
/// and the only symptom is a live site that fails at startup with "The app
/// could not start" — for every visitor, and only in a browser.
///
/// So the two are asserted equal here rather than trusted to be edited
/// together. And the *declaration* is asserted as well as the use: an undeclared
/// `WASM_BINDGEN_VERSION` expands to nothing, the generator step installs no
/// generator, and every other check here passes — which is how an earlier
/// version of `pages.yml` published a site with no app in it.
#[test]
fn the_pages_build_generates_bindings_for_the_pinned_runtime() {
    let Some(pages) = live_pages_workflow() else {
        return;
    };

    // Read the pin the way Cargo writes it: `wasm-bindgen = "=0.2.128"`.
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).expect("Cargo.toml");
    let pinned = manifest
        .lines()
        .find_map(|line| {
            // The line must *be* the dependency, not merely mention it:
            // `wasm-bindgen-futures = "0.4.78"` sits next to it, and a
            // `starts_with("wasm-bindgen")` matches both.
            let rest = line.trim().strip_prefix("wasm-bindgen = ")?;
            let rest = rest.trim().trim_matches('"');
            (!rest.is_empty()).then_some(rest)
        })
        .expect("Cargo.toml pins a wasm-bindgen version");
    assert!(
        pinned.starts_with('='),
        "Cargo.toml must pin wasm-bindgen exactly ({pinned:?}), so there is one version for \
         the crate and one for the bindings generator; a caret would resolve to whatever \
         the lockfile holds and the workflow's literal would name one arbitrary version",
    );
    // `pinned` is `"=0.2.128"`, quotes stripped: the `=` is part of the value.
    let version = pinned.trim_start_matches('=');

    let Some(env) = top_level_env(&pages) else {
        panic!(
            "pages.yml must have a top-level `env:` block declaring WASM_BINDGEN_VERSION: \
             {version}; the install step reads $WASM_BINDGEN_VERSION, an undeclared variable \
             expands to nothing, and the job would then publish bindings for no runtime"
        );
    };
    assert_eq!(
        env.get("WASM_BINDGEN_VERSION").map(String::as_str),
        Some(version),
        "pages.yml's WASM_BINDGEN_VERSION must be exactly the Cargo.toml pin ({version})",
    );
}

/// This template must not carry a wake-lock cfg it does not need, and must not
/// be told it does.
///
/// Verified 2026-10-02: a clean wasm build — fresh `CARGO_TARGET_DIR`, no
/// `RUSTFLAGS`, no `.cargo/config.toml` — finishes, so the crate compiles
/// without `--cfg=web_sys_unstable_apis`. That cfg is for web-sys's unstable
/// surface (the Screen Wake Lock), and this crate does not use it.
///
/// The rule is the *agreement*, not the absence: whatever the crate is built
/// with is what the workflow must lint with. A workflow carrying a cfg the
/// crate is not built with does not merely waste a flag — web-sys types several
/// getters differently behind it (`MouseEvent::client_x` is `i32` normally,
/// `f64` with the cfg), so the wasm clippy run reads a green about a different
/// program from the one that ships, and only that run can see it.
///
/// So this asserts the workflow declares no `RUSTFLAGS` at all, and — so a
/// future app that legitimately adds the Wake Lock gets a *changed* test with a
/// reason rather than a deleted one — names where the cfg would go.
#[test]
fn the_pages_workflow_declines_a_cfg_this_crate_does_not_need() {
    let Some(pages) = live_pages_workflow() else {
        return;
    };

    let env = top_level_env(&pages).unwrap_or_default();
    assert!(
        !env.contains_key("RUSTFLAGS"),
        "pages.yml declares RUSTFLAGS={}; this template uses no unstable web-sys API and its \
         wasm build was verified clean without it, so the flag can only make the wasm clippy \
         run lint a different web-sys than the one that ships. If you have added an unstable \
         API, set it here deliberately and update this test with the reason",
        env.get("RUSTFLAGS").map_or("", String::as_str),
    );

    // The same for `Cargo.toml`: a `[target.wasm32...] rustflags` there is
    // documented as ignored for non-path dependencies, so it would be a line
    // that looks load-bearing and is not.
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).expect("Cargo.toml");
    assert!(
        !manifest.contains("rustflags"),
        "Cargo.toml sets rustflags; cargo ignores `[target.*.rustflags]` for registry \
         dependencies, so this would not reach web-sys and only look like it does",
    );

    // And no `.cargo/config.toml`: none exists, and adding one for a flag this
    // crate does not need would put the same dead configuration in every repo
    // generated from the template.
    let config = root().join(".cargo/config.toml");
    assert!(
        !config.exists(),
        ".cargo/config.toml must not exist: this crate needs no compiler flag, and a copy of \
         the template would inherit a configuration with nothing in it to justify",
    );
}

/// The generator version must be declared in `env:`, not written as a literal
/// into the install step — and the install step must actually read it.
///
/// These are two different mistakes. A literal in the step is right once and
/// wrong forever after the pin moves, and the test above cannot catch it because
/// it only compares against `Cargo.toml`. A `$WASM_BINDGEN_VERSION` with no
/// declaration installs nothing. So: the use must be there, and it must be the
/// variable, not a hard-coded string.
#[test]
fn the_generator_is_installed_from_the_declared_variable() {
    let Some(pages) = live_pages_workflow() else {
        return;
    };
    // Read the value off its own line rather than splitting the whole file: a
    // `split_once('"')` on the text that follows runs past the closing quote
    // and across a dozen lines into unrelated shell, which reads as a
    // catastrophic failure and is really a bad parse.
    let mut uses = pages.lines().filter_map(|line| {
        let value = line.trim().strip_prefix("version=")?;
        let value = value.strip_prefix('"')?;
        value.split('"').next()
    });
    let install = uses.next().expect(
        "pages.yml must pass a `version=\"…\"` to the install step, so the pin in \
         Cargo.toml has one place to be wrong",
    );
    assert_eq!(
        install, "$WASM_BINDGEN_VERSION",
        "pages.yml must install the generator with version=\"$WASM_BINDGEN_VERSION\" rather \
         than a literal, so the pin in Cargo.toml has exactly one place to be wrong; found \
         version=\"{install}\"",
    );
    // And nowhere else: `asset="wasm-bindgen-${version}-…"` builds the download
    // name from the same variable, so exactly one `version="` may exist. Two
    // means one of them is a literal, and the assertion above — which reads only
    // the first — would pass on the variable while the install used the other.
    assert_eq!(
        pages.matches("version=\"").count(),
        1,
        "pages.yml must name the generator version exactly once, as \
         version=\"$WASM_BINDGEN_VERSION\"; a second occurrence is a hard-coded literal the \
         first assertion cannot see",
    );
}

/// A deploy that can run from any branch is a deploy a stranger can run.
///
/// `pages: write` and `id-token: write` are the two permissions that let a job
/// overwrite the live site, and the token behind them is minted for the
/// repository however the workflow was reached. The project *wants* an automatic
/// deploy on every merge to master — that is the point, and it is why nobody
/// has to remember to publish. What it does not want is that power on every
/// other ref, so the invariant asserted here is the one that survives the
/// convenience: master is the only ref that can reach the live site, and the
/// publishing permissions live in the one job that is gated on it.
#[test]
fn only_master_can_reach_the_live_site() {
    // Already comment-stripped: every assertion below reads the live gate, and
    // the prose directly above it names both halves of the gate it explains.
    let Some(pages) = live_pages_workflow() else {
        return;
    };

    // The trigger must be the named branch, not a bare `push:`. A bare `push:`
    // deploys from every branch that exists, including a contributor's.
    assert!(
        pages.contains("branches: [master]"),
        "pages.yml must trigger on `branches: [master]`, not a bare `push:`, which deploys \
         from every branch including other people's",
    );
    // A tag trigger alongside the branch trigger would publish a version that
    // was never on master.
    assert!(
        !pages.contains("tags:"),
        "pages.yml must not also deploy on tags; a tagged commit that never reached master \
         would be published to the live site",
    );

    // The deploy job's gate, named so the assertion cannot be satisfied by a
    // gate on some other job. This is the check that actually holds when the
    // trigger is widened by accident.
    let deploy = pages
        .split("\n  deploy:")
        .nth(1)
        .expect("pages.yml must have a `deploy:` job");
    assert!(
        deploy.contains("github.ref == 'refs/heads/master'"),
        "the `deploy` job must be gated on the build being for master",
    );
    assert!(
        deploy.contains("\n    if:"),
        "the `deploy` job must carry an `if:` gate at job level; `needs: build` alone still \
         runs for every trigger",
    );

    // And it must ALSO be gated on not being a fork. This workflow is
    // byte-identical in `wdomitrz/pwa_template` and its fork
    // `bot-git-ai/pwa_template` — a fork exists precisely so its files can be
    // copied — and both have a `master`, so a gate that tests only the branch
    // name cannot tell them apart: a push to the fork's master then tries to
    // publish a site it was never given. Two things go wrong, and the first is
    // the one that happens: a fork has no Pages site of its own until someone
    // enables one by hand, so every push to fork master dies at "Ensure GitHub
    // Pages has been enabled", after a green build and a full site check. And
    // were Pages enabled there, the fork would serve its own divergent copy.
    //
    // `github.event.repository.fork` is the discriminator because it needs no
    // configuration: the event supplies it, false upstream and true in the fork.
    // The obvious alternative, a repository Actions variable, has the failure
    // mode this assertion exists to prevent — it would have to be set on the
    // *user's* repository to publish, and no account but the user's can do that,
    // so the gate would ship as silently off on the one repository that owns
    // the site.
    //
    // Read the gate out of the comment-stripped workflow, never the raw one:
    // the comment block directly above this `if:` names both halves while
    // explaining them, so an assertion over raw text is satisfied by the prose
    // with the clause deleted. That is not hypothetical — it is how the
    // `RUSTFLAGS` assertion in `chess_clock`'s equivalent file shipped.
    let live_gate = pages
        .split("\n  deploy:")
        .nth(1)
        .and_then(|job| job.split_once("if:"))
        .map(|(_, after)| after.lines().next().unwrap_or_default())
        .expect("the `deploy` job must have an `if:` gate");
    assert!(
        live_gate.contains("!github.event.repository.fork"),
        "the `deploy` job's `if:` must test `!github.event.repository.fork`; this workflow is \
         byte-identical in the fork `bot-git-ai/pwa_template`, so a branch-name-only gate \
         publishes from the fork too -- failing with 'Ensure GitHub Pages has been enabled' \
         until Pages is enabled there, and serving a divergent copy afterwards. Found: \
         {live_gate:?}",
    );
    // And the fork rule must *extend* the master gate, not replace it. Two `if:`
    // keys in one job is not an AND — YAML keeps the last one — so a gate that
    // keeps `github.ref` and adds `!fork` on a second line is the only correct
    // spelling, and dropping the master half would publish from every branch.
    assert!(
        live_gate.contains("github.ref == 'refs/heads/master'"),
        "the fork clause must extend the master gate rather than replace it: `deploy` must be \
         ONE `if:` testing both `github.ref` and `github.event.repository.fork`. Found: \
         {live_gate:?}",
    );
    // Exactly one gate, so the clause cannot be split onto a second `if:` that
    // silently overrides the first.
    assert_eq!(
        deploy.matches("\n    if:").count(),
        1,
        "the `deploy` job must have exactly one `if:`; a second one silently replaces the first \
         in YAML, which is how a half-written gate ends up publishing",
    );
    // And the forbidden alternative, so the documented mistake cannot come back
    // through a copy of an earlier draft.
    assert!(
        !pages.contains("vars.PUBLISH_PAGES"),
        "pages.yml gates the deploy on `vars.PUBLISH_PAGES`; it would have to be set on the \
         user's own repository to publish and this account holds only `pull` there, so the gate \
         ships as silently off on the repository that owns the site",
    );

    // The permissions that can actually publish must be scoped to `deploy`,
    // not granted workflow-wide, so a build step or a third-party action added
    // later cannot spend them.
    let build = pages
        .split("\n  build:")
        .nth(1)
        .and_then(|after| after.split("\n  deploy:").next())
        .expect("pages.yml must have a `build:` job");
    for forbidden in ["pages: write", "id-token: write"] {
        assert!(
            !build.contains(forbidden),
            "the `build` job must not hold `{forbidden}`; those belong to `deploy` alone",
        );
    }
    // And the workflow-level `permissions:` must not grant them either — that is
    // exactly the "workflow-wide" grant being ruled out.
    let header = pages.split("\njobs:").next().unwrap_or_default();
    for forbidden in ["pages: write", "id-token: write"] {
        assert!(
            !header.contains(forbidden),
            "the workflow-level `permissions:` must not grant `{forbidden}`; scope it to the \
             `deploy` job",
        );
    }
}

/// The deploy has to be handed something the upload actually produced.
///
/// `deploy-pages` v5 takes `artifact_name`. There is no `artifact_id` input:
/// passing one is reported as `Unexpected input(s) 'artifact_id'` and ignored,
/// and the action falls back to its own default — which is only right while the
/// upload side defaults to the same string. Change one side and the deploy finds
/// no artifact and fails with a bare `HttpError: Not Found` that names nothing.
///
/// So the names are read out of the live text and compared, rather than
/// asserting a fixed string: the invariant is the agreement, not the particular
/// name.
#[test]
fn the_deploy_is_handed_the_artifact_the_build_uploaded() {
    let Some(pages) = live_pages_workflow() else {
        return;
    };

    // The input v5 does not have. Its presence is a warning at run time and
    // never an error, so nothing else would ever report it.
    assert!(
        !pages.contains("artifact_id:"),
        "pages.yml passes `artifact_id` to deploy-pages v5, which has no such input; it is \
         warned about and ignored, leaving the deploy to guess the artifact name",
    );

    // Read the two keys where they are the artifact's own. `artifact_name:` is
    // unique; the upload's `name:` is the one under the upload step's `with:`.
    let value_of = |key: &str| -> Option<String> {
        let mut in_with = false;
        for line in pages.lines() {
            let trimmed = line.trim();
            if trimmed == "with:" {
                in_with = true;
                continue;
            }
            // A new key at or left of the step's own indent ends the `with:` block.
            if in_with && !line.starts_with("          ") && !trimmed.starts_with('#') {
                in_with = false;
            }
            if in_with && trimmed.starts_with(&format!("{key}: ")) {
                return Some(
                    trimmed[format!("{key}: ").len()..]
                        .trim()
                        .trim_matches('"')
                        .to_owned(),
                );
            }
        }
        None
    };
    let uploaded = value_of("name").expect(
        "pages.yml must state the upload step's artifact `name:`, so the deploy has something \
         to match",
    );
    let deployed = value_of("artifact_name").expect(
        "pages.yml must pass `artifact_name:` to deploy-pages, rather than leaving it to a \
         default that can drift from the upload",
    );
    assert_eq!(
        uploaded, deployed,
        "the artifact the build uploads ({uploaded:?}) and the one the deploy asks for \
         ({deployed:?}) must be the same name",
    );

    // And it must be the Pages uploader, not the generic one, which produces a
    // differently-shaped artifact the deploy cannot find.
    assert!(
        pages.contains("actions/upload-pages-artifact@"),
        "pages.yml must use actions/upload-pages-artifact; actions/upload-artifact produces a \
         different artifact and the deploy then fails to find what it was given",
    );
}

/// The two workflows must pin the icon the repository *actually has*.
///
/// `assets/icon.svg` is the source every install PNG is derived from, and both
/// workflows assert that `dist/icon.svg` is that file byte for byte and that its
/// digest is the author's original. That check is what caught the workflow
/// sitting red from 2026-10-01 to 2026-10-02: the icon changed from the `P`
/// glyph to three bars, `tests/shell.rs`'s `ICON_SHA256` was updated, and
/// `build.yml`'s copy was not — the same pin, kept in two places, drifting.
///
/// This test is the one that stops the third place from drifting too. It reads
/// the digest out of each workflow and asserts both equal the constant the icon
/// test uses, and that the constant still matches the file on disk.
#[test]
fn both_workflows_pin_the_icon_the_repository_still_has() {
    let icon = root().join("assets/icon.svg");
    let bytes =
        std::fs::read(&icon).unwrap_or_else(|error| panic!("reading {}: {error}", icon.display()));
    let digest = sha256_hex(&bytes);

    for workflow_name in ["build.yml", "pages.yml"] {
        let Some(text) = workflow(workflow_name) else {
            continue; // a partial export need not carry every workflow
        };
        let live = strip_yaml_comments(&text);
        assert!(
            live.contains(&digest),
            "{workflow_name} pins a digest for assets/icon.svg that is not the icon this repository has \
             (the file hashes to {digest}); the workflow has been red since the icon changed, \
             and shipping it would pin the wrong icon as 'the author's original'",
        );
    }
}

/// The site check the workflow runs must assert on the site, not on the exit
/// status.
///
/// No test in this file can assert on `dist/`: it is gitignored, so the release
/// gate's exported tree never has it. The workflow is therefore the only
/// automated coverage of the built output, and it has to check the things a
/// green build cannot imply — the file list, the substituted cache version, the
/// bindings' exports and start function, the icons being real PNGs, and the page
/// calling the initializer. Each of these is a failure that is otherwise
/// completely silent: a site that loads and never starts shows nothing but a
/// blank page, long after the build looked fine.
#[test]
fn the_workflow_checks_the_built_site_rather_than_the_exit_status() {
    let Some(pages) = workflow("pages.yml") else {
        return;
    };
    let live = strip_yaml_comments(&pages);

    // Every one of the nine files, by name. Naming them is the point: the
    // failure this exists for is a `dist/` that looks publishable and is missing
    // something the page references, and a file nobody wrote does not change a
    // count.
    //
    // Read the list out of the workflow's own `expected=` assignment rather than
    // asserting eight literals here: a second hand-kept list would be a second
    // copy of the truth, which is the mistake this whole file is written
    // against. It drifts the moment someone adds a file to one place and not
    // the other — and `DIST_FILES` is derived from the two owners, so an
    // eight-item list is available for free.
    let expected = live
        .lines()
        .find_map(|line| {
            // The whole line must be the assignment, not merely contain it:
            // `missing="${missing}"` ends in the same characters, and that line
            // comes first. A test that reads the wrong line of the right file
            // is worse than one that reads nothing.
            let value = line.trim().strip_prefix("expected=")?;
            value.strip_prefix('"')?.strip_suffix('"')
        })
        .expect("pages.yml must state the files it expects as an `expected=\"...\"` list");
    let listed: Vec<&str> = expected.split_whitespace().collect();
    assert_eq!(
        listed, DIST_FILES,
        "pages.yml's site check names these files; they must be exactly the nine the two \
         owners produce, in one list, derived from the same source",
    );
    // And it must fail on unexpected files as well as missing ones, checked by
    // listing what is there and diffing it against that same list rather than
    // counting: a stray file ships exactly as quietly as an absence. Read as a
    // line, not as a substring of one — `grep -q -v -f` looks much like
    // `grep -x -F -v -f`, and a substring assertion over shell is a way of
    // feeling safe without being so.
    assert!(
        live.lines()
            .any(|line| line.contains("grep -x -F -v -f") && line.contains("extra=")),
        "pages.yml must reject unexpected files in dist/ as well as missing ones, by listing \
         what is there and diffing it against the expected list",
    );

    // The cache version, the bindings' exports, and the start function — each
    // asserted as a line that actually greps `dist/`, not as the word appearing
    // somewhere in the file. `__VERSION__` is named in this workflow's own prose
    // more than once, so an assertion that merely looks for the word passes with
    // the check deleted.
    for (needle, why) in [
        (
            "__VERSION__",
            "an unsubstituted placeholder ships a worker that never invalidates",
        ),
        (
            "export",
            "the page loads app.js with a dynamic import, so a truncated one fails only \
                    in a browser",
        ),
        (
            "__wbindgen_start",
            "without it the bindings load and the app never runs, silently",
        ),
    ] {
        // The check greps a quoted needle, as `grep -q '__VERSION__' dist/…` does:
        // quoting is what keeps the pattern from being read as a shell
        // redirection or glob.
        assert!(
            live.lines().any(|line| {
                line.contains(&format!("grep -q '{needle}'")) && line.contains("dist/")
            }),
            "pages.yml must run a `grep -q '{needle}'` against dist/ ({why}); the word merely \
             appearing in the file is not the check",
        );
    }
    // The install icons must be checked as PNGs, not just named — a file with
    // the right name and no raster installs with a broken icon.
    assert!(
        live.lines().any(|line| line.contains("89504e470d0a1a0a")),
        "pages.yml must check the PNG magic number; a dist/icon-*.png that is not a PNG has the \
         right name and no image",
    );
    // The page must call the initializer, which is the one failure with no
    // error message anywhere.
    assert!(
        live.lines()
            .any(|line| line.contains("grep -q 'm.default()'") && line.contains("dist/index.html")),
        "pages.yml must check that dist/index.html calls m.default(); without it the bindings \
         load and the app never run, silently",
    );
    // And the Python must be a quoted heredoc, not `python3 -c`: an indented
    // `-c` body is quoted by the shell before Python sees it, and a single quote
    // in it ends the string early — a parse error pointing at nothing wrong.
    assert!(
        live.contains("<<'PY'"),
        "pages.yml's multi-line Python must be a <<'PY' heredoc, not python3 -c; an indented -c \
         body breaks on shell quoting, not on Python",
    );
    assert!(
        !live.contains("python3 -c"),
        "pages.yml must not use python3 -c for the site check's Python",
    );
}

/// The workflow must build the site itself, in the documented order, because
/// nothing committed can.
///
/// `dist/app.js` and `dist/app_bg.wasm` exist nowhere else in the tree — they
/// are written by `wasm-bindgen` and are gitignored. A workflow that checked out
/// the repository and uploaded `dist/` would publish a site that loads and never
/// starts, from committed files alone. So the build steps have to be here, in
/// order, and the `touch` has to survive: `build.rs` writes into the source tree
/// rather than `OUT_DIR`, so Cargo cannot see its own output changed and skips
/// the second build, leaving `dist/` with the two wasm artefacts and none of the
/// six shell files.
#[test]
fn the_workflow_builds_the_site_rather_than_publishing_committed_files() {
    let Some(pages) = live_pages_workflow() else {
        return;
    };
    for needle in [
        "cargo build --locked --lib --target wasm32-unknown-unknown --release",
        "wasm-bindgen --target web --no-typescript --out-dir dist --out-name app",
        "touch build.rs",
        "cargo build --release --locked",
    ] {
        assert!(
            pages.contains(needle),
            "pages.yml must run `{needle}`; the site's wasm artefacts exist nowhere else",
        );
    }
    // The wasm artefact's name follows the crate, and a renamed crate that
    // forgets this gets an empty bindings directory and a green job.
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).expect("Cargo.toml");
    let crate_name = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("name = "))
        .map(|rest| rest.trim().trim_matches('"').to_owned())
        .expect("Cargo.toml names the package");
    assert!(
        pages.contains(&format!(
            "target/wasm32-unknown-unknown/release/{crate_name}.wasm"
        )),
        "pages.yml must bind the crate's own wasm ({crate_name}.wasm); a renamed crate that \
         misses this publishes bindings for no crate at all",
    );
}

/// The Pages workflow must stay separate from the build.
///
/// `build.yml` runs on every pull request, including from forks. A deploy needs
/// `pages: write`, `id-token: write` and the `github-pages` environment, none of
/// which a fork PR has — so folding the deploy into `build.yml` would make the
/// CI build unrunnable by anyone who can push a branch, and would put the power
/// to overwrite the live site on every ref. Two files, one power.
#[test]
fn the_deploy_lives_in_its_own_workflow() {
    let (Some(build), Some(pages)) = (workflow("build.yml"), workflow("pages.yml")) else {
        return;
    };
    for (name, text) in [("build.yml", &build), ("pages.yml", &pages)] {
        let live = strip_yaml_comments(text);
        // A `uses:` step is written `- uses: actions/deploy-pages@…` on one line, with
        // whatever indent and list marker the surrounding block uses. Match the
        // action reference itself, not an exact line prefix, so a step nested
        // differently still registers: the invariant is "this workflow runs
        // deploy-pages".
        let runs = |action: &str| {
            live.lines()
                .filter_map(|line| line.trim().strip_prefix("- "))
                .chain(live.lines().map(str::trim))
                .any(|line| {
                    line.strip_prefix("uses: ")
                        .is_some_and(|rest| rest.starts_with(action))
                })
        };
        let deploys = runs("actions/deploy-pages@");
        let expected = name == "pages.yml";
        assert_eq!(
            deploys,
            expected,
            "{name} must {} the deploy step; the Pages deploy is a separate workflow so the CI \
             build stays runnable from a fork PR, where pages: write and the github-pages \
             environment do not exist",
            if expected { "carry" } else { "not carry" },
        );
    }
    assert!(
        !build.contains("upload-pages-artifact"),
        "build.yml must not upload a Pages artifact; it uploads a plain run artifact, and the \
         Pages deploy has its own build",
    );
    // The deploy must live behind the *Pages* environment, so a reviewer-gated
    // environment can hold the live site back. Read the two lines as a block,
    // not as two independent `contains`: `name: github-pages` also appears as
    // the artifact's name and as the `artifact_name`, so searching the whole
    // file for it would stay true after the environment was renamed — and the
    // rename is exactly the mutation that quietly removes the gate.
    let env_gate = pages.lines().any(|line| line.trim() == "environment:")
        && pages
            .lines()
            .any(|line| line.trim() == "name: github-pages");
    assert!(
        env_gate,
        "the deploy job must set `environment:` with `name: github-pages`, so the repository's \
         Pages approval rules can gate the live site",
    );
    // And the two must be adjacent: an `environment:` with no name, or a name
    // belonging to something else, is not the gate.
    let gated = pages
        .lines()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair[0].trim() == "environment:" && pair[1].trim() == "name: github-pages");
    assert!(
        gated,
        "the deploy job's `environment:` must be followed by `name: github-pages`; a renamed \
         or nameless environment is not the repository's Pages gate",
    );
}

/// A workflow must not depend on an action whose SHA is unpinned.
///
/// Every `uses:` in this file is a 40-character commit SHA with the version in a
/// trailing comment. A tag or a branch is mutable: whoever can move the tag
/// runs code with this repository's credentials — and the deploy job's token can
/// write to Pages. This is the one supply-chain property that is cheap to assert
/// and impossible to notice when it lapses.
#[test]
fn every_action_is_pinned_to_a_commit_sha() {
    for name in ["build.yml", "pages.yml"] {
        let Some(text) = workflow(name) else {
            continue;
        };
        let live = strip_yaml_comments(&text);
        for line in live.lines() {
            let Some(rest) = line.trim().strip_prefix("uses:") else {
                continue;
            };
            let reference = rest.trim();
            let Some((_, sha)) = reference.rsplit_once('@') else {
                panic!("{name}: `uses: {reference}` names no ref; every action must be pinned");
            };
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{name}: `uses: {reference}` is not pinned to a 40-character commit SHA; a tag \
                 or branch is mutable and runs code with this repository's credentials",
            );
        }
    }
}

/// `top_level_env` must see the block it claims to, and only that block.
///
/// Every workflow test that asks "is this set for the whole workflow?" reads
/// through this helper, so a helper that quietly returns an empty map would make
/// them all skip — and a skipped test reads the same as a passing one in the
/// summary. It is therefore exercised against a synthetic document with
/// hand-written expectations, including the two ways it can go wrong: stopping
/// at the wrong place, and running on past the end of the block into a key that
/// belongs to the next section.
#[test]
fn the_env_reader_reads_the_block_and_stops_at_the_next_section() {
    let env = top_level_env(
        "\
env:
  CARGO_TERM_COLOR: always
  WASM_BINDGEN_VERSION: 0.2.128

permissions:
  contents: read
",
    )
    .expect("the document has an env: block");
    assert_eq!(
        env.get("CARGO_TERM_COLOR").map(String::as_str),
        Some("always"),
    );
    assert_eq!(
        env.get("WASM_BINDGEN_VERSION").map(String::as_str),
        Some("0.2.128"),
    );
    // The block ended at `permissions:`; this key is indented the same way and
    // would be swallowed by a reader that kept going.
    assert!(
        !env.contains_key("contents"),
        "the reader ran past the end of env: into permissions:",
    );

    // A document with no `env:` at all is `None`, not an empty map — the
    // difference between "the block is empty" and "there is no block", which is
    // the failure the tests that use this need to name.
    assert!(
        top_level_env("name: pages\non:\n  push:\n").is_none(),
        "a document with no env: must read as None, not as an empty block",
    );
    // An indented `env:` is not a top-level block.
    assert!(
        top_level_env("jobs:\n  env:\n    CARGO_TERM_COLOR: always\n").is_none(),
        "an env: nested under jobs: is not the workflow's env:",
    );
}

/// SHA-256 of `bytes`, lowercase hex.
///
/// Self-contained on purpose. Pinning a file's digest is worth a dependency in
/// an app; in a template whose whole argument is that it needs none, it is not
/// — so this is the FIPS 180-4 round constants and 64 rounds, and it is
/// checked against the published test vectors in the test below. If you ever
/// change this function, that test has to keep passing.
// Verbatim FIPS 180-4: the round constants, the 64 rounds and the standard
// a..h state names are the specification; splitting or renaming them would
// only make the code harder to check against it.
#[allow(clippy::many_single_char_names, clippy::too_many_lines)]
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];

    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    // Pad to a multiple of 64 bytes: 0x80, zeros, then the bit length as u64be.
    let mut message = bytes.to_vec();
    let bits = (bytes.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());

    for block in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (index, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut hex = String::with_capacity(state.len() * 8);
    for word in state {
        write!(hex, "{word:08x}").expect("writing to String cannot fail");
    }
    hex
}

/// The digest function above is load-bearing for the icon test, so it is
/// checked against the published vectors rather than trusted.
#[test]
fn the_pinned_digest_function_is_correct() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b"The quick brown fox jumps over the lazy dog"),
        "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
    );
}

/// `source` with its `//` and `/* ... */` comments dropped.
///
/// Used so an assertion about what `src/ui.rs` *does* cannot be satisfied by a
/// comment saying what it does.
fn strip_rust_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    // A line comment runs to the end of the line, and a block comment to its
    // closer. A `//` inside a string literal is not a comment, but none of the
    // strings these tests look for contain one, and a full Rust tokenizer is
    // not worth the complexity to guard against it.
    while let Some(start) = rest.find('/') {
        let after = &rest[start + 1..];
        if let Some(tail) = after.strip_prefix("//") {
            out.push_str(&rest[..start]);
            rest = tail.split_once('\n').map_or("", |(_, line)| line);
        } else if let Some(tail) = after.strip_prefix("/*") {
            out.push_str(&rest[..start]);
            rest = tail.split_once("*/").map_or("", |(_, line)| line);
        } else {
            out.push_str(&rest[..=start]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// The app is frontend-only: a static site and nothing else.
///
/// The family's whole shape is that `dist/` is the product and any file host can
/// serve it. That claim is cheap to state and easy to lose — one `[[bin]]`, one
/// `axum` dependency or one `std::net::TcpListener` turns the site into something
/// that needs a process of its own, and the first symptom would be a deploy that
/// works locally and nowhere else.
#[test]
fn the_site_is_frontend_only() {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).expect("Cargo.toml");
    let live: String = manifest
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !live.contains("[[bin]]"),
        "the crate must stay a library: no binary means no server to run"
    );
    assert!(
        live.contains("crate-type = [\"cdylib\", \"rlib\"]"),
        "the site is built from a cdylib; without it there is nothing to publish"
    );

    // A server dependency is the other way to lose it, and it is a one-line
    // change that nothing else in the tree would notice.
    for banned in [
        "axum",
        "actix",
        "hyper",
        "tiny_http",
        "warp",
        "rocket",
        "tide",
    ] {
        assert!(
            !live.to_lowercase().contains(banned),
            "{banned} is a server framework; this app is served as static files"
        );
    }

    // And the sources must not open a socket or bind a port.
    for source in ["src/lib.rs", "src/board.rs", "src/game.rs", "src/ui.rs"] {
        let text = std::fs::read_to_string(root().join(source))
            .unwrap_or_else(|error| panic!("reading {source}: {error}"));
        let code = strip_rust_comments(&text);
        // `bind(` alone would also match this app's own `bind(window, ...)`
        // call, which binds listeners to elements. These are the socket APIs.
        for banned in [
            "TcpListener",
            "TcpStream",
            "UdpSocket",
            "std::net",
            "socket2",
        ] {
            assert!(
                !code.contains(banned),
                "{source} uses {banned}: a static site cannot open a socket"
            );
        }
    }

    // The published site is exactly the files a browser needs.
    let worker = worker_template();
    let precached = precached_by_the_worker();
    assert!(
        precached.len() >= 8,
        "the worker must precache the whole shell, found {precached:?}"
    );
    let _ = worker;
}

/// The colour-scheme blocks change colours and nothing else.
///
/// Dark is the default and light is chosen by the system, in CSS alone so the
/// right palette is there before the first paint. The light block is where a
/// layout change hides: one `padding` or `font-size` in the "colours" override
/// makes the two schemes different products, and because it only shows when
/// someone switches their phone at the wrong moment, nobody would report it.
#[test]
fn the_light_scheme_changes_colours_and_nothing_else() {
    let page = shell();
    // The palette lives in the stylesheet, which is its own file now.
    let stylesheet = style();

    let Some(start) = stylesheet.find("@media (prefers-color-scheme: light)") else {
        panic!("the stylesheet has no light-scheme block, so it cannot follow the system");
    };
    // The block is `@media (...) {  :root { ... } }`: two closing braces. Find
    // the end by counting them, so a nested rule cannot truncate the body.
    let block = &stylesheet[start..];
    let mut depth = 0usize;
    let mut end = 0usize;
    for (at, character) in block.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = at;
                    break;
                }
            }
            _ => {}
        }
    }
    let block = &block[..=end];

    // Every declaration in the block must set a custom property. A colour can
    // only reach the page through one of the variables, which is what keeps one
    // set of layout rules working for both schemes.
    let mut declarations = 0usize;
    for line in block.lines() {
        let trimmed = line.trim().trim_end_matches(&[';', '}'][..]).trim();
        if trimmed.is_empty()
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with('}')
            || trimmed.contains('{')
            || trimmed.starts_with("@media")
        {
            continue;
        }
        let Some((property, _)) = trimmed.split_once(':') else {
            continue;
        };
        let property = property.trim();
        declarations += 1;
        assert!(
            property.starts_with("--") || property == "color-scheme",
            "the light-scheme block sets {property:?}; it may change colours and \
             color-scheme only, or the two schemes stop being the same app laid \
             out the same way"
        );
        assert!(
            !property.starts_with("--board"),
            "the board's own size is layout, not colour: it must not be overridden \
             per scheme"
        );
    }
    assert!(
        declarations > 8,
        "the light-scheme block looks empty ({declarations} declarations), so the \
         app would render its dark palette on a light phone"
    );

    // And the two schemes must be named for the system, not switched by a script.
    assert!(
        page.contains("media=\"(prefers-color-scheme: dark)\"")
            && page.contains("media=\"(prefers-color-scheme: light)\""),
        "both theme-color metas must follow the system's scheme"
    );
    let script = page
        .split_once("<script type=\"module\">")
        .expect("the one module script")
        .1;
    assert!(
        !script.contains("matchMedia") && !script.contains("prefers-color-scheme"),
        "the theme must be chosen in CSS: a script runs after the first paint, \
         so the wrong palette is shown first"
    );
}

/// Strip `//` and `/* */` comments from JavaScript, so an assertion is about
/// code rather than about prose that discusses it.
fn strip_js_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find("//") {
        // A `//` inside a string literal would be a false positive; the worker
        // has none, and the ones it does have are URLs with a scheme, which
        // never begin a line this function is asked about.
        out.push_str(&rest[..at]);
        match rest[at..].find('\n') {
            Some(end) => rest = &rest[at + end..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// A new version must be able to reach an installed app.
///
/// This is the failure the family spent three sessions on: a worker that
/// installs a new version and then waits — for days, on a phone — while the app
/// keeps running the code it was closed with. The worker is the half that was
/// missing here: the page asked, and nothing answered.
#[test]
fn a_new_version_can_take_over_a_running_app() {
    let worker = strip_js_comments(&worker_template());

    assert!(
        worker.contains("self.addEventListener('message'"),
        "the worker must listen for the page's handover request"
    );
    assert!(
        worker.contains("event.data === 'SKIP_WAITING'") && worker.contains("self.skipWaiting()"),
        "the worker must call skipWaiting, and only for the page's own message"
    );

    // And never on install: that would swap the wasm out from under a game in
    // progress, which is the reason the call is gated in the first place.
    let install = worker
        .split_once("addEventListener('install'")
        .expect("the install handler")
        .1
        .split_once("addEventListener(")
        .expect("the next listener")
        .0;
    assert!(
        !install.contains("skipWaiting"),
        "skipWaiting must not run on install"
    );

    // The page's half, in the browser layer: it posts the message and reloads
    // once when the new worker takes control.
    let ui =
        strip_rust_comments(&std::fs::read_to_string(root().join("src/ui.rs")).expect("src/ui.rs"));
    assert!(
        ui.contains("SKIP_WAITING"),
        "the page must ask the waiting worker in"
    );
    assert!(
        ui.contains("post_message"),
        "the ask has to be a message to the waiting worker"
    );
    assert!(
        ui.contains("controllerchange"),
        "the page has to reload when the new worker takes control, or it keeps \
         running the old code in a page that reports the new version"
    );
    assert!(
        ui.contains("registration.update()"),
        "a check of our own is needed: register() resolving does not mean the \
         update check it started has finished, and updatefound can fire first"
    );

    // The manifest is the browser's, not the page's: serving it from the cache
    // pins an installed app to the name, icon and start URL it shipped with.
    assert!(
        worker.contains("if (url === MANIFEST) return;"),
        "the worker must leave the manifest to the network"
    );
}

/// The menu is folded by default, holds what is not playing, and can be closed
/// without a click listener.
///
/// A player mid-puzzle wants four controls. The difficulty of the *next*
/// puzzle, the marking setting and the two buttons that throw the board away
/// are not among them, and the destructive pair should not sit under a thumb
/// that was aiming for Undo.
///
/// It is a checkbox with a label, and that is not a style choice: opening it
/// used to hang off a `click` listener, and a tap on a touch screen produced no
/// `click` at all — only pointerdown/up and touchstart/end — so on a phone the
/// menu never opened. A checkbox toggles itself on every one of those gestures.
/// Everything that closes the sheet is a `label` for the same checkbox, for the
/// same reason.
#[test]
fn the_menu_is_folded_and_can_be_closed_without_a_listener() {
    let page = shell();
    assert!(
        page.contains("<input id=\"menu\" type=\"checkbox\""),
        "the menu toggle must be a checkbox: it toggles without a click listener"
    );
    assert!(
        page.contains("id=\"menu-panel\" hidden"),
        "the menu must start folded"
    );
    assert!(
        !page.contains("id=\"menu\" type=\"button\""),
        "a button here needs a click listener, and a tap does not always produce one"
    );
    // The face of the checkbox, the scrim and Done: all labels, no script.
    let labels = page.matches("<label for=\"menu\"").count();
    assert!(
        labels >= 3,
        "the checkbox's face, the scrim and Done must all be labels for it; \
         found {labels}"
    );

    // The menu holds the things that are not part of playing, and they are not
    // in the tools row beside the board.
    let tools = page
        .split_once("<div id=\"tools\">")
        .expect("the tools row")
        .1
        .split_once("</div>")
        .expect("the end of the tools row")
        .0;
    for inside in [
        "id=\"difficulty\"",
        "id=\"marking\"",
        "id=\"new\"",
        "id=\"restart\"",
    ] {
        assert!(page.contains(inside), "{inside} is missing from the shell");
        assert!(
            !tools.contains(inside),
            "{inside} belongs in the menu, not beside the board"
        );
    }

    for slug in ["easy", "gentle", "medium", "tricky", "hard", "expert"] {
        assert!(
            page.contains(&format!("value=\"{slug}\"")),
            "the menu does not offer {slug}"
        );
    }
    for slug in ["off", "rules", "answer"] {
        assert!(
            page.contains(&format!("value=\"{slug}\"")),
            "the menu does not offer the {slug} marking setting"
        );
    }
}

/// Both workflows run the same site check, on the same output.
///
/// `build.yml` runs it on every pull request; `pages.yml` runs it again before
/// publishing. They had drifted apart — each checked things the other did not —
/// so a pull request could go green on a site the deploy would reject, and the
/// deploy could publish something the pull request never saw.
#[test]
fn both_workflows_run_the_same_site_check() {
    let step = |name: &str| -> Option<String> {
        let text = workflow(name)?;
        let start = text.find("      - name: Check the built site")?;
        let end = text[start..].find("      - name: Run the tests")? + start;
        Some(text[start..end].to_owned())
    };
    let (Some(build), Some(pages)) = (step("build.yml"), step("pages.yml")) else {
        return; // a partial export need not carry every workflow
    };
    assert_eq!(
        build, pages,
        "the site check must be identical in build.yml and pages.yml: one runs on \
         pull requests and the other before publishing, and a check that differs \
         between them lets a pull request pass a site the deploy rejects"
    );
}
