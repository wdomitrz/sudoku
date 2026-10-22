//! The browser layer: the only file that talks to the DOM.
//!
//! Every decision — which digit is where, whether it is wrong, what a pencil
//! mark does, what the clock says, what is worth saving — was made in
//! [`crate::board`] and [`crate::game`], and is unit tested there. What is left
//! here is element handles, attribute writes, listeners and a timer, and
//! nothing that could have been written down as a rule.
//!
//! Three things this file does that the template it came from did not, each for
//! a reason worth stating:
//!
//! * **One listener per group, not one per cell.** A 9×9 grid is 81 buttons and
//!   a number pad is nine; attaching a closure to each would be 90 forgotten
//!   closures and 90 chances for one of them to be attached twice. The grid has
//!   a single listener that reads the cell index off the element the event
//!   landed on.
//! * **A cell is built once and then only described.** Creating 81 buttons and
//!   their pencil-mark slots on every render would throw away focus, restart
//!   any tap in progress, and cost more than the update does; so the elements
//!   are made at startup and each render rewrites what changed.
//! * **The clock redraws the clock.** A second passing changes one line, so
//!   [`App::tick`] paints the header and leaves the board alone.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    Document, Element, Event, HtmlButtonElement, HtmlSelectElement, KeyboardEvent, Storage, Window,
};

use crate::board::{Difficulty, Digits, CELLS};
use crate::game::{Game, Marking, Report};

/// The app's whole mutable state, and the element handles it writes through.
///
/// `Rc<RefCell<_>>` because wasm has no threads and every handler below is a
/// plain closure rather than a `'static` thread. The rule that keeps this
/// honest: a handler borrows, mutates, drops the borrow, then renders. It never
/// renders from inside a borrow.
type Shared = Rc<RefCell<App>>;

/// The scope this app's worker is registered for.
///
/// Stated rather than inherited, and the one string that has to agree with
/// `src/service-worker.js`: the worker resolves its own directory the same way,
/// from `self.location`.
const SCOPE: &str = "./";

/// How often the clock is advanced.
const TICK_MS: i32 = 1_000;

/// How many pencil-mark slots each cell holds, one per digit.
const NOTE_SLOTS: usize = 9;

/// Start the app.
///
/// Runs automatically when the page's dynamic import resolves, because the
/// loader in `ui.html` is `import('./app.js').then(m => m.default())` and
/// `default` is this start function.
///
/// # Errors
///
/// Fails only when the document is not there at all — a browser without a
/// `window` or a `document` is not a page this app can run in. A missing
/// element inside the shell degrades a feature rather than failing startup.
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;

    let app: Shared = Rc::new(RefCell::new(App::build(&window, &document)?));
    {
        let mut app = app.borrow_mut();
        app.paint();
    }
    bind(&window, &document, &app);
    start_clock(&window, &app);

    // By value, not by reference: the future outlives this function, so it has
    // to own what it holds. This is the same rule the event closures follow.
    let offline = window.clone();
    spawn_local(keep_offline(offline));
    Ok(())
}

/// Everything one running app holds.
struct App {
    window: Window,
    document: Document,
    game: Game,
    /// The difficulty the *next* puzzle will be, which is what the select shows.
    /// It is deliberately not the current game's difficulty on restore: the
    /// select is a request, and changing it must not discard a game in progress.
    next: Difficulty,
    cells: Vec<HtmlButtonElement>,
    /// The element each cell's digit is written into, so a digit and a pencil
    /// mark can coexist without either erasing the other.
    digit_slots: Vec<Element>,
    /// The hidden element holding each cell's spoken name.
    labels: Vec<Element>,
    note_slots: Vec<Vec<Element>>,
    /// Whether the folded menu is open. The panel is a view concern — nothing
    /// about the game changes when it opens — so it lives here rather than in
    /// `Game`, and it is deliberately not remembered between visits: a menu that
    /// was left open would cover the board on the next launch.
    menu_open: bool,
    /// The pencil marks last written to each cell, so a render can skip the
    /// ~700 element writes that unchanged marks would otherwise cost.
    shown_notes: [Digits; CELLS],
}

impl App {
    /// Build the app: restore a game if there is one, otherwise start a fresh
    /// puzzle, and create the elements that are created once.
    ///
    /// # Errors
    ///
    /// When the shell has no board to build on, or the browser refuses to
    /// create an element. Both are "this page cannot host the app", not a
    /// recoverable state.
    fn build(window: &Window, document: &Document) -> Result<Self, JsValue> {
        let restored = stored_game(window);
        let game = restored.unwrap_or_else(|| Game::start(Difficulty::Easy, fresh_seed()));
        let next = game.difficulty();

        let board = element(document, "board").ok_or_else(|| JsValue::from_str("no #board"))?;
        let mut cells = Vec::with_capacity(CELLS);
        let mut digit_slots = Vec::with_capacity(CELLS);
        let mut labels = Vec::with_capacity(CELLS);
        let mut note_slots = Vec::with_capacity(CELLS);
        for index in 0..CELLS {
            let cell = document
                .create_element("button")?
                .dyn_into::<HtmlButtonElement>()?;
            cell.set_attribute("type", "button")?;
            cell.set_attribute("data-index", &index.to_string())?;
            // Roving tabindex: the grid is one stop in the tab order, not 81,
            // and the arrow keys move within it from there.
            cell.set_tab_index(if index == 0 { 0 } else { -1 });
            // The digit gets its own element. Writing it as the button's own
            // text would destroy every child the button has — which is where
            // the pencil marks live — and the loss is invisible until someone
            // marks a cell that has just had a digit erased from it.
            let digit_element = document.create_element("span")?;
            digit_element.set_class_name("digit");
            // The digit and the pencil marks are the *same* information the
            // spoken label carries, in a form a screen reader would read out as
            // a bare number with no idea where it is. They are hidden from the
            // accessibility tree and the label below is the cell's only name.
            digit_element.set_attribute("aria-hidden", "true")?;
            cell.append_child(&digit_element)?;
            let notes = document.create_element("span")?;
            notes.set_class_name("notes");
            notes.set_attribute("aria-hidden", "true")?;
            let mut slots = Vec::with_capacity(NOTE_SLOTS);
            for _ in 0..NOTE_SLOTS {
                let slot = document.create_element("i")?;
                notes.append_child(&slot)?;
                slots.push(slot);
            }
            cell.append_child(&notes)?;
            board.append_child(&cell)?;
            // The cell's own name, as an element rather than an attribute on
            // the control: a hidden span is a real piece of text that the
            // browser, a translation and a speech engine all read the same way,
            // and it is the one form the shell's own markup never hard-codes.
            let label = document.create_element("span")?;
            label.set_class_name("visually-hidden");
            cell.append_child(&label)?;
            labels.push(label);
            digit_slots.push(digit_element);
            cells.push(cell);
            note_slots.push(slots);
        }

        build_pad(document)?;

        Ok(Self {
            window: window.clone(),
            document: document.clone(),
            game,
            next,
            cells,
            digit_slots,
            labels,
            note_slots,
            menu_open: false,
            shown_notes: [0; CELLS],
        })
    }

    /// Paint everything.
    fn paint(&mut self) {
        self.paint_board();
        self.paint_header();
        self.paint_controls();
    }

    /// Paint the 81 cells: their digit, their pencil marks, their class tokens
    /// and their spoken name.
    fn paint_board(&mut self) {
        let cells = self.game.cells();
        let selected = self.game.selected();
        for (index, cell) in cells.iter().enumerate() {
            let element = &self.cells[index];
            element.set_class_name(&cell.classes());
            let label = &self.labels[index];
            let wanted = cell.label();
            if element_text(label) != wanted {
                label.set_text_content(Some(&wanted));
            }
            element.set_tab_index(if Some(index) == selected { 0 } else { -1 });

            // A digit and pencil marks never show together: a cell holding a
            // digit has no marks (the model clears them), so the two are drawn
            // independently and cannot collide.
            let text = if cell.value == 0 {
                String::new()
            } else {
                cell.value.to_string()
            };
            let digit_element = &self.digit_slots[index];
            if element_text(digit_element) != text {
                digit_element.set_text_content(Some(&text));
            }

            // Only touch the pencil marks when they changed. A digit placed on
            // one side of the board must not rewrite 700 elements on the other.
            if self.shown_notes[index] != cell.notes {
                for (slot_index, slot) in self.note_slots[index].iter().enumerate() {
                    let digit = u8::try_from(slot_index + 1).unwrap_or(0);
                    let wanted = if cell.notes & crate::board::bit(digit) != 0 {
                        digit.to_string()
                    } else {
                        String::new()
                    };
                    if element_text(slot) != wanted {
                        slot.set_text_content(Some(&wanted));
                    }
                }
                self.shown_notes[index] = cell.notes;
            }
        }
    }

    /// Paint the clock, the mistake count and what is left.
    fn paint_header(&mut self) {
        let readout = self.game.readout();
        let document = &self.document;
        set_text(document, "clock", &readout.clock);
        set_text(document, "stamp", self.game.difficulty().label());
        if self.game.is_won() {
            set_text(document, "solved-detail", &readout.summary);
        }
    }

    /// Paint the controls whose state is part of the game: notes mode, the undo
    /// and hint availability, the solved panel and the difficulty select.
    fn paint_controls(&mut self) {
        let document = self.document.clone();
        if let Some(notes) = button(&document, "notes") {
            let _ = notes.set_attribute(
                "aria-pressed",
                if self.game.notes_mode() {
                    "true"
                } else {
                    "false"
                },
            );
        }
        if let Some(undo) = button(&document, "undo") {
            undo.set_disabled(!self.game.can_undo());
        }
        if let Some(erase) = button(&document, "erase") {
            erase.set_disabled(self.game.selected().is_none());
        }
        if let Some(panel) = html_element(&document, "solved") {
            panel.set_hidden(!self.game.is_won());
        }
        if let Some(pad) = html_element(&document, "pad") {
            pad.set_hidden(self.game.is_won());
        }
        if let Some(tools) = html_element(&document, "tools") {
            tools.set_hidden(self.game.is_won());
        }
        for (id, wanted) in [
            ("difficulty", self.next.slug()),
            ("marking", self.game.marking().slug()),
        ] {
            if let Some(select) = document
                .get_element_by_id(id)
                .and_then(|element| element.dyn_into::<HtmlSelectElement>().ok())
            {
                if select.value() != wanted {
                    select.set_value(wanted);
                }
            }
        }
    }

    /// Advance the clock, repainting the header only.
    ///
    /// A second is a small change and the board is not part of it. Painting the
    /// whole grid sixty times a minute would throw away the pencil marks' own
    /// diffing for no gain, and on a slow phone it is a visible stutter.
    fn tick(&mut self) {
        self.game.tick();
        self.paint_header();
    }

    /// Say what just happened, out loud and on the line below the board.
    ///
    /// The reader can see which cell they touched; telling them it was row 4,
    /// column 7 is a coordinate they have no use for, on the one line that could
    /// carry something worth reading. So the line reports the *effect* — the
    /// digit written, the pencil mark added, the clash — and says nothing at all
    /// about a plain selection, which changes nothing they cannot see.
    fn announce(&mut self, report: Report) {
        set_text(&self.document, "status", &report.words());
    }

    /// Persist the game, if it is worth persisting.
    fn save(&self) {
        let Some(storage) = self.storage() else {
            return;
        };
        if let Err(error) = storage.set_item(crate::game::STORAGE_KEY, &self.game.to_stored()) {
            warn("could not save the game", &error);
        }
    }

    fn storage(&self) -> Option<Storage> {
        self.window.local_storage().ok().flatten()
    }

    /// Show or hide the panel to match the checkbox.
    ///
    /// The checkbox owns the state; this only draws it. The panel is pinned to
    /// the bottom of the screen, so opening it moves nothing — no scroll, and no
    /// button sliding out from under the reader's thumb between the tap that
    /// opened it and the tap that closes it.
    fn set_menu_open(&mut self, open: bool) {
        self.menu_open = open;
        if let Some(panel) = html_element(&self.document, "menu-panel") {
            panel.set_hidden(!open);
        }
    }

    /// Replace the game and paint it from scratch.
    ///
    /// The pencil-mark cache is cleared first: a new puzzle's marks are all
    /// empty, and thinking they are already painted would leave the previous
    /// puzzle's marks on the board.
    fn replace(&mut self, game: Game) {
        self.game = game;
        self.shown_notes = [0; CELLS];
        self.paint();
    }

    /// Apply a move, then repaint and save it.
    fn apply(&mut self, report: Report) {
        self.announce(report);
        self.paint();
        self.save();
    }
}

/// Start the clock.
///
/// One interval for the life of the page, rather than one per game: a timer per
/// puzzle would need cancelling on every new game and would leak the ones that
/// were not. The game itself decides what a tick means — a solved or paused game
/// ignores it.
fn start_clock(window: &Window, app: &Shared) {
    let shared = Rc::clone(app);
    let closure = Closure::<dyn FnMut()>::new(move || {
        shared.borrow_mut().tick();
    });
    if window
        .set_interval_with_callback_and_timeout_and_arguments_0(
            closure.as_ref().unchecked_ref(),
            TICK_MS,
        )
        .is_err()
    {
        warn(
            "the clock could not start",
            &JsValue::from_str("no interval"),
        );
    }
    closure.forget();
}

/// Make the nine number-pad buttons, once.
fn build_pad(document: &Document) -> Result<(), JsValue> {
    let pad = element(document, "pad").ok_or_else(|| JsValue::from_str("no #pad"))?;
    for digit in 1..=9u8 {
        let button = document
            .create_element("button")?
            .dyn_into::<HtmlButtonElement>()?;
        button.set_attribute("type", "button")?;
        button.set_attribute("data-digit", &digit.to_string())?;
        // The digit and nothing else. It used to carry a count of how many of
        // that digit were left, which reads as a scoreboard beside a puzzle
        // rather than a keypad — and it had to be kept right on every move.
        button.set_text_content(Some(&format!("{digit}")));
        pad.append_child(&button)?;
    }
    Ok(())
}

/// Bind every listener.
///
/// Handlers mutate the state, then repaint. That is the whole application loop:
/// no framework, no virtual DOM, no diffing.
fn bind(window: &Window, document: &Document, app: &Shared) {
    bind_grid(document, app);
    bind_pad(document, app);
    bind_tools(document, app);
    bind_menu(document, app);
    bind_keyboard(window, app);

    // Save on the way out. A phone suspends a backgrounded tab without warning —
    // often by killing it — and a game lost to that is a game the player had
    // every reason to expect back.
    if let Some(document) = window.document() {
        let shared = Rc::clone(app);
        let closure = Closure::<dyn FnMut()>::new(move || {
            shared.borrow().save();
        });
        listen(
            &document,
            "visibilitychange",
            closure.as_ref().unchecked_ref(),
        );
        closure.forget();
    }
}

/// The grid: one listener for 81 cells.
///
/// The cell is resolved from the element the event landed on, which is why the
/// cells carry their index as an attribute rather than capturing it in a
/// closure each.
fn bind_grid(document: &Document, app: &Shared) {
    let Some(board) = element(document, "board") else {
        warn("the shell has no #board", &JsValue::from_str("missing"));
        return;
    };
    let shared = Rc::clone(app);
    let closure = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        let Some(index) = cell_index_from(&event) else {
            return;
        };
        let mut app = shared.borrow_mut();
        let report = app.game.select(Some(index));
        app.announce(report);
        app.paint();
    });
    listen(&board, "click", closure.as_ref().unchecked_ref());
    closure.forget();
}

/// The number pad: one listener for nine digits.
fn bind_pad(document: &Document, app: &Shared) {
    let Some(pad) = element(document, "pad") else {
        warn("the shell has no #pad", &JsValue::from_str("missing"));
        return;
    };
    let shared = Rc::clone(app);
    let closure = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        let Some(digit) = digit_from(&event) else {
            return;
        };
        let mut app = shared.borrow_mut();
        let report = app.game.enter(digit);
        app.apply(report);
        // A tap should leave the grid focused, so the next arrow key works.
        app.focus_selected();
    });
    listen(&pad, "click", closure.as_ref().unchecked_ref());
    closure.forget();
}

/// The four tools, which differ only in which move they make.
fn bind_tools(document: &Document, app: &Shared) {
    for (id, tool) in [
        ("notes", Tool::Notes),
        ("undo", Tool::Undo),
        ("erase", Tool::Erase),
    ] {
        let Some(button) = button(document, id) else {
            warn("the shell is missing a control", &JsValue::from_str(id));
            continue;
        };
        let shared = Rc::clone(app);
        let closure = Closure::<dyn FnMut()>::new(move || {
            let mut app = shared.borrow_mut();
            let report = match tool {
                Tool::Notes => app.game.toggle_notes(),
                Tool::Undo => app.game.undo(),
                Tool::Erase => app.game.erase(),
            };
            app.apply(report);
            // Notes is a mode, not a move: it must not move the keyboard away
            // from the cell the reader is working on.
            if !matches!(tool, Tool::Notes) {
                app.focus_selected();
            }
        });
        listen(&button, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

/// The new-puzzle, restart and difficulty controls.
fn bind_menu(document: &Document, app: &Shared) {
    // Two buttons start a new puzzle: the one in the menu, and the one in the
    // solved panel so the reader does not have to find the menu behind it.
    for id in ["new", "solved-new"] {
        let Some(button) = button(document, id) else {
            continue;
        };
        let shared = Rc::clone(app);
        let closure = Closure::<dyn FnMut()>::new(move || {
            let mut app = shared.borrow_mut();
            let difficulty = app.next;
            app.replace(Game::start(difficulty, fresh_seed()));
            app.save();
            set_text(&app.document, "status", "New puzzle.");
        });
        listen(&button, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    // The menu is a checkbox: the browser toggles it on any tap, drag-cancelled
    // tap, key press or screen-reader activation, with no listener of ours. All
    // this does is follow the box and keep the panel, the styling and the
    // scroll position in step with it.
    if let Some(toggle) = document
        .get_element_by_id("menu")
        .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        let shared = Rc::clone(app);
        let input = toggle.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            shared.borrow_mut().set_menu_open(input.checked());
        });
        listen(&toggle, "change", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    if let Some(restart) = button(document, "restart") {
        let shared = Rc::clone(app);
        let closure = Closure::<dyn FnMut()>::new(move || {
            let mut app = shared.borrow_mut();
            let report = app.game.restart();
            // Every pencil mark is gone, and the cache of what is on screen has
            // to know that or the old marks would stay painted.
            app.shown_notes = [0; CELLS];
            app.apply(report);
        });
        listen(&restart, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    if let Some(select) = document
        .get_element_by_id("marking")
        .and_then(|element| element.dyn_into::<HtmlSelectElement>().ok())
    {
        let shared = Rc::clone(app);
        let closure = Closure::<dyn FnMut(Event)>::new(move |_| {
            let mut app = shared.borrow_mut();
            if let Some(select) = app
                .document
                .get_element_by_id("marking")
                .and_then(|element| element.dyn_into::<HtmlSelectElement>().ok())
            {
                let marking = Marking::from_slug(&select.value()).unwrap_or(Marking::Rules);
                let report = app.game.set_marking(marking);
                app.apply(report);
            }
        });
        listen(&select, "change", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    if let Some(select) = document
        .get_element_by_id("difficulty")
        .and_then(|element| element.dyn_into::<HtmlSelectElement>().ok())
    {
        let shared = Rc::clone(app);
        let closure = Closure::<dyn FnMut(Event)>::new(move |_| {
            // Only the *next* puzzle changes. Discarding the game in progress
            // because someone looked at another difficulty would be the kind of
            // data loss a person never forgives an app for.
            let mut app = shared.borrow_mut();
            if let Some(select) = app
                .document
                .get_element_by_id("difficulty")
                .and_then(|element| element.dyn_into::<HtmlSelectElement>().ok())
            {
                app.next = Difficulty::from_slug(&select.value()).unwrap_or(Difficulty::Easy);
                set_text(&app.document, "status", "");
            }
        });
        listen(&select, "change", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

/// The keys a physical keyboard adds, because a phone is not the only way in.
///
/// Digits and the arrow keys are what the grid implies; `n` and `u` are the two
/// tools whose names a reader can remember.
fn bind_keyboard(window: &Window, app: &Shared) {
    let shared = Rc::clone(app);
    let closure = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
        // A key pressed inside the select or a focused button is the browser's
        // business before it is ours.
        if is_text_entry(&event) {
            return;
        }
        let key = event.key();
        let handled = {
            let mut app = shared.borrow_mut();
            match key.as_str() {
                "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" => {
                    let Some(next) = neighbour(app.game.selected(), &key) else {
                        return;
                    };
                    let report = app.game.select(Some(next));
                    app.announce(report);
                    app.paint();
                    app.focus_selected();
                    true
                }
                "Backspace" | "Delete" | "0" => {
                    let report = app.game.erase();
                    app.apply(report);
                    true
                }
                "n" | "N" => {
                    let report = app.game.toggle_notes();
                    app.apply(report);
                    true
                }
                "u" | "U" => {
                    let report = app.game.undo();
                    app.apply(report);
                    true
                }
                "Escape" => {
                    let report = app.game.select(None);
                    app.announce(report);
                    app.paint();
                    true
                }
                digit => match digit.parse::<u8>() {
                    Ok(value) if (1..=9).contains(&value) => {
                        let report = app.game.enter(value);
                        app.apply(report);
                        true
                    }
                    _ => false,
                },
            }
        };
        if handled {
            event.prevent_default();
        }
    });
    listen(window, "keydown", closure.as_ref().unchecked_ref());
    closure.forget();
}

/// Whether a keystroke is destined for a control rather than the grid.
fn is_text_entry(event: &KeyboardEvent) -> bool {
    let Some(target) = event.target() else {
        return false;
    };
    let Ok(element) = target.dyn_into::<Element>() else {
        return false;
    };
    matches!(element.tag_name().as_str(), "INPUT" | "SELECT" | "TEXTAREA")
}

/// The cell an arrow key moves to, or `None` when there is nowhere to go.
fn neighbour(from: Option<usize>, key: &str) -> Option<usize> {
    use crate::board::{column_of, row_of};
    let index = from.unwrap_or(0);
    let (row, column) = (row_of(index), column_of(index));
    let (row, column) = match key {
        "ArrowLeft" => (row, column.checked_sub(1)?),
        "ArrowRight" => (row, (column + 1 < 9).then_some(column + 1)?),
        "ArrowUp" => (row.checked_sub(1)?, column),
        _ => ((row + 1 < 9).then_some(row + 1)?, column),
    };
    Some(row * 9 + column)
}

impl App {
    /// Put the keyboard on the selected cell.
    ///
    /// Only ever called from a handler the user drove with a key or a tap on
    /// the grid: focusing on load would scroll the board into view and put a
    /// focus ring on a cell nobody chose.
    fn focus_selected(&self) {
        if let Some(index) = self.game.selected() {
            if let Some(cell) = self.cells.get(index) {
                let _ = cell.focus();
            }
        }
    }
}

/// Keep the app working with no connection.
///
/// Three things have to happen, and they happen in this order because each one
/// depends on the last being true: register the worker, let a *new* worker take
/// over when it is safe, and ask the browser to keep the cached copy on disk.
async fn keep_offline(window: Window) {
    match register_worker(&window).await {
        Ok(container) => release_stale_registrations(&window, &container).await,
        Err(error) => warn("offline support is unavailable", &error),
    }
    keep_storage(&window).await;
}

/// Register the worker, and hand it a reload when a new version is ready.
///
/// # Errors
///
/// When the browser refuses the registration: no worker support, a scope it
/// will not grant, or a script that fails to fetch. Offline support is then
/// absent and the app is otherwise unaffected, which is why this is a warning
/// rather than a failed start.
async fn register_worker(window: &Window) -> Result<web_sys::ServiceWorkerContainer, JsValue> {
    let container = window.navigator().service_worker();

    // The scope is stated rather than inherited: left to itself, a
    // registration's scope is the directory of the page that registered it —
    // right today, and silently wrong once the app is published elsewhere. A
    // worker registered for the whole origin answers for every page on it,
    // including pages that are not this app.
    let options = web_sys::RegistrationOptions::new();
    options.set_scope(SCOPE);
    let registration =
        JsFuture::from(container.register_with_options("./service-worker.js", &options))
            .await?
            .dyn_into::<web_sys::ServiceWorkerRegistration>()?;

    // A worker that was already waiting — a deploy that happened while the app
    // was closed — is the first case, and the one a phone hits most often.
    ask_waiting_worker_in(&registration);

    // The second case: this visit finds the update. `updatefound` fires, the
    // installing worker reaches `installed`, and only then is it waiting.
    let registration_for_update = registration.clone();
    let update_found = Closure::<dyn FnMut()>::new(move || {
        let Some(installing) = registration_for_update.installing() else {
            return;
        };
        let waiting_registration = registration_for_update.clone();
        // The listener needs the worker too, so it gets its own handle rather
        // than borrowing the one the attachment below consumes.
        let watched = installing.clone();
        let on_state = Closure::<dyn FnMut()>::new(move || {
            if watched.state() == web_sys::ServiceWorkerState::Installed {
                ask_waiting_worker_in(&waiting_registration);
            }
        });
        let _ = installing
            .add_event_listener_with_callback("statechange", on_state.as_ref().unchecked_ref());
        // The closure has to outlive the listener it was given to. It is fired
        // at most twice (installing → installed) and then replaced on the next
        // update, so holding it is the point rather than a leak worth avoiding.
        on_state.forget();
    });
    let _ = registration
        .add_event_listener_with_callback("updatefound", update_found.as_ref().unchecked_ref());
    update_found.forget();

    // …and, because the two cases above are both races the browser can lose, a
    // check of our own. `register()` resolving does not mean the update check it
    // started has finished, so an update that arrives a moment later fires
    // `updatefound` before the listener above exists — the new worker then sits
    // in `waiting` for as long as the app stays open, which is exactly how an
    // installed app stops receiving fixes. Asking for a check and looking again
    // when it resolves closes that window.
    if let Ok(promise) = registration.update() {
        let _ = JsFuture::from(promise).await;
    }
    ask_waiting_worker_in(&registration);

    // And the handover itself: when the new worker takes control, the page is
    // still the old version in memory, so reload — exactly once, or the page
    // would loop.
    let reloaded = Rc::new(std::cell::Cell::new(false));
    let window = window.clone();
    let controller = Closure::<dyn FnMut()>::new(move || {
        if reloaded.replace(true) {
            return;
        }
        let _ = window.location().reload();
    });
    let _ = container
        .add_event_listener_with_callback("controllerchange", controller.as_ref().unchecked_ref());
    controller.forget();

    Ok(container)
}

/// Hand this app's own URLs back to the current worker.
///
/// A registration is kept by the browser, not by the tab, so it outlives the
/// page that made it and keeps answering for its scope until something
/// explicitly unregisters it. That is how a page on this origin comes to be
/// served by a worker installed for a *different* app, long after that app was
/// closed — and a reload, a newer version or a newer worker cannot fix it,
/// because the newer worker only takes control where its own scope reaches.
///
/// So: unregister any registration whose scope covers this app's directory but
/// is not this app's directory. Failures are ignored — this is best-effort
/// cleanup of state this app did not create, and a browser that refuses leaves
/// the reader no worse off.
async fn release_stale_registrations(window: &Window, container: &web_sys::ServiceWorkerContainer) {
    let Ok(registrations) = JsFuture::from(container.get_registrations()).await else {
        return;
    };
    let Ok(array) = registrations.dyn_into::<js_sys::Array>() else {
        return;
    };

    // This app's own directory, as an absolute URL with a trailing slash.
    let Ok(home) = window.location().href() else {
        return;
    };
    let Ok(ours) = web_sys::Url::new_with_base(&home, "./") else {
        return;
    };
    let ours = ours.href();

    for entry in array.iter() {
        let Ok(registration) = entry.dyn_into::<web_sys::ServiceWorkerRegistration>() else {
            continue;
        };
        let scope = registration.scope();
        // Leave alone any scope that is this app's own, or narrower: a sibling
        // app mounted inside this directory is legitimate and separate, and
        // nothing there can intercept us. One test, because the two cases are
        // the same one: `ours` begins with `scope`.
        if ours.starts_with(&scope) {
            continue;
        }
        // What is left is a scope that is a *strict* prefix of ours: a worker
        // that would be consulted for this app's URLs while registered for more
        // of the origin. Only our own worker qualifies — a different app's
        // worker lives in a different directory, so unregistering it would break
        // the app it belongs to.
        let script = registration
            .active()
            .map(|worker| worker.script_url())
            .unwrap_or_default();
        if script_belongs_to_app(&script, &ours) {
            match registration.unregister() {
                Ok(promise) => {
                    let _ = JsFuture::from(promise).await;
                }
                Err(error) => warn("stale service worker could not be released", &error),
            }
        }
    }
}

/// Whether the worker script at `script` is this app's own worker, registered
/// for more of the origin than this app's directory.
///
/// A wider scope means the script sits at the root of this app's own directory
/// rather than below it: a sibling app's worker is in a sibling directory and
/// does not match. `strip_suffix`, not `trim_end_matches` — the latter strips a
/// *set of characters*, so it would eat a directory named `...e-worker.js` and
/// call it ours.
fn script_belongs_to_app(script: &str, ours: &str) -> bool {
    match web_sys::Url::new(script) {
        Ok(url) => url.href().strip_suffix("service-worker.js") == Some(ours),
        Err(_) => false,
    }
}

/// Ask a waiting worker to take over.
///
/// The message is the whole protocol: the worker's `skipWaiting` answers this
/// and nothing else, so an update never swaps the app's code under a live tab
/// — only a page that has decided it can be reloaded asks for the swap.
fn ask_waiting_worker_in(registration: &web_sys::ServiceWorkerRegistration) {
    if let Some(waiting) = registration.waiting() {
        if let Err(error) = waiting.post_message(&JsValue::from_str("SKIP_WAITING")) {
            warn("could not hand over to the new version", &error);
        }
    }
}

/// Ask the browser not to evict this app's cached files.
///
/// Cache Storage is evictable by default: under storage pressure a browser may
/// drop the copy that makes the app work offline, and the reader finds out on a
/// train. The request is a request — a browser may decline, and some do — so a
/// refusal is a normal condition and not something to report on the page.
async fn keep_storage(window: &Window) {
    let storage = window.navigator().storage();
    let Ok(promise) = storage.persist() else {
        return;
    };
    let _ = JsFuture::from(promise).await;
}

/// The game saved by a previous visit, or `None`.
///
/// Every failure — no `localStorage`, no entry, unreadable text, a line written
/// by an older version — is the same answer: start a fresh puzzle. A corrupt
/// entry must never be able to stop the app from starting.
fn stored_game(window: &Window) -> Option<Game> {
    let storage = window.local_storage().ok().flatten()?;
    let raw = storage.get_item(crate::game::STORAGE_KEY).ok().flatten()?;
    Game::from_stored(&raw)
}

/// A seed for the generator.
///
/// Milliseconds since the epoch, which is new on every tap and needs no
/// randomness source. The cast truncates the fractional part deliberately: a
/// seed one millisecond apart is a different puzzle, and a reader cannot tap
/// twice in one.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn fresh_seed() -> u64 {
    let now = js_sys::Date::now();
    if now.is_finite() && now >= 0.0 {
        now as u64
    } else {
        1
    }
}

/// The digit a pad click is about.
fn digit_from(event: &Event) -> Option<u8> {
    let element = event.target()?.dyn_into::<Element>().ok()?;
    let button = element.closest("button").ok().flatten()?;
    button
        .get_attribute("data-digit")
        .and_then(|text| text.parse::<u8>().ok())
}

/// The cell a grid click is about.
fn cell_index_from(event: &Event) -> Option<usize> {
    let element = event.target()?.dyn_into::<Element>().ok()?;
    let button = element.closest("button").ok().flatten()?;
    button
        .get_attribute("data-index")
        .and_then(|text| text.parse::<usize>().ok())
}

/// Which tool a control is.
#[derive(Clone, Copy)]
enum Tool {
    Notes,
    Undo,
    Erase,
}

/// The text of an element, or `""`.
fn element_text(element: &Element) -> String {
    element.text_content().unwrap_or_default()
}

/// Attach a listener, reporting rather than panicking when it cannot.
///
/// `window`, `document` and an element are three different types with the same
/// event interface, which is why this casts to `EventTarget` rather than taking
/// one concrete type. A target that is somehow not one is a warning, not a
/// panic: the app still runs without that listener.
fn listen<T: JsCast>(target: &T, event: &str, callback: &js_sys::Function) {
    let Some(target) = target.dyn_ref::<web_sys::EventTarget>() else {
        warn("not an event target", &JsValue::from_str(event));
        return;
    };
    if let Err(error) = target.add_event_listener_with_callback(event, callback) {
        warn("could not bind an event listener", &error);
    }
}

/// A document element, or `None`.
///
/// Never panics: the shell is a separate file that someone will edit, and a
/// missing id should degrade the app, not abort it.
fn element(document: &Document, id: &str) -> Option<Element> {
    document.get_element_by_id(id)
}

/// The typed form of [`element`], so button methods are available without a cast.
fn button(document: &Document, id: &str) -> Option<HtmlButtonElement> {
    element(document, id)?.dyn_into().ok()
}

/// The typed form of [`element`], for the parts of the shell that are hidden.
fn html_element(document: &Document, id: &str) -> Option<web_sys::HtmlElement> {
    element(document, id)?.dyn_into().ok()
}

/// Replace an element's text.
fn set_text(document: &Document, id: &str, text: &str) {
    if let Some(element) = element(document, id) {
        element.set_text_content(Some(text));
    }
}

/// Report something recoverable. Never a panic, never silent.
fn warn(context: &str, error: &JsValue) {
    web_sys::console::warn_2(&JsValue::from_str(context), error);
}
