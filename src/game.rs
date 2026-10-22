//! The game: one puzzle as the player is working through it.
//!
//! No `web-sys`, no `js-sys`, no `wasm-bindgen`, so this module compiles and
//! runs natively under `cargo test`. Where [`crate::board`] is the puzzle, this
//! is the sitting: what is selected, what has been written down, pencil marks,
//! how long it has taken, what can be taken back.
//!
//! Two rules carry most of the design:
//!
//! * Every mutation goes through a snapshot. Undo is then correct by
//!   construction rather than by remembering, for each feature, what it
//!   changed — the mistake that shows up later as "undo brought the digit back
//!   but left the pencil marks missing".
//! * Every word the app shows about the game is written here, in one place.
//!   The screen paints what this module says; it never composes a sentence of
//!   its own.

use crate::board::{
    bit, column_of, digits_of, generate, peers, row_of, solve, Difficulty, Digits, Grid, CELLS,
};

/// Key the game is saved under.
///
/// One prefix per app, so two PWAs on the same origin cannot read or overwrite
/// each other's state. The version is in the stored value, not the key: a key
/// that changes loses every game in progress.
pub const STORAGE_KEY: &str = "sudoku.game";

/// How many moves can be taken back.
///
/// Each entry is a whole-board snapshot (about 330 bytes), so this cap is the
/// difference between a bounded few tens of kilobytes and a slow leak in a tab
/// someone leaves open all day.
pub const HISTORY_LIMIT: usize = 200;

/// The stored format's version.
const VERSION: &str = "v1";

/// How many hex digits a pencil-mark mask is stored as.
///
/// Nine bits do not fit in two hex digits, and a truncated mask would come back
/// as a different set of pencil marks rather than as an error.
const NOTES_WIDTH: usize = 3;

/// One cell, as the screen needs to draw it.
///
/// Derived, never stored: [`Game::cells`] computes all 81 in one pass because
/// whether a cell is a peer of the selection — and whether one of its digits is
/// in conflict — are questions about the whole board, not about the cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // a view record: six independent facts
pub struct Cell {
    /// Which cell this is, in reading order.
    pub index: usize,
    /// The digit in it, or `0`.
    pub value: u8,
    /// Pencil marks, as a bit mask.
    pub notes: Digits,
    /// Whether the puzzle put the digit there.
    pub given: bool,
    /// Whether the digit is not the answer's.
    pub wrong: bool,
    /// Whether the digit repeats in its row, column or box.
    pub conflict: bool,
    /// Whether this is the selected cell.
    pub selected: bool,
    /// Whether this cell shares a row, column or box with the selection.
    pub peer: bool,
    /// Whether this cell holds the same digit as the selected cell.
    pub same: bool,
}

impl Cell {
    /// The cell's class tokens: the visual state, as one attribute.
    ///
    /// One `class` write per cell per render rather than six `data-*` writes.
    /// The tokens are computed from the state here and nowhere else, so the
    /// stylesheet cannot become the place that decides what is wrong.
    #[must_use]
    pub fn classes(&self) -> String {
        let mut tokens: Vec<&str> = Vec::with_capacity(7);
        if self.given {
            tokens.push("given");
        }
        if self.value != 0 && !self.given {
            tokens.push("player");
        }
        if self.wrong {
            tokens.push("wrong");
        }
        if self.conflict {
            tokens.push("conflict");
        }
        if self.peer {
            tokens.push("peer");
        }
        if self.same {
            tokens.push("same");
        }
        if self.selected {
            tokens.push("selected");
        }
        tokens.join(" ")
    }

    /// What a screen reader should hear for this cell.
    ///
    /// The visible text of a sudoku cell is one digit, which never says where
    /// it is or whether the puzzle put it there — so a cell is the one control
    /// in this app whose accessible name has to be written out. Position first,
    /// because a grid is scanned by position.
    #[must_use]
    pub fn label(&self) -> String {
        use std::fmt::Write as _;

        let (row, column) = (row_of(self.index) + 1, column_of(self.index) + 1);
        let mut label = format!("Row {row}, column {column}");
        if self.value == 0 {
            if self.notes == 0 {
                label.push_str(", empty.");
            } else {
                label.push_str(", empty, notes");
                for digit in digits_of(self.notes) {
                    let _ = write!(label, " {digit}");
                }
                label.push('.');
            }
        } else if self.wrong {
            // Contested before the value: a screen reader should say what is
            // wrong with the cell before reading out what was put in it.
            let _ = write!(label, ", {}, wrong.", self.value);
        } else {
            let _ = write!(label, ", {}.", self.value);
        }
        if self.conflict {
            label.push_str(" Repeats in its row, column or box.");
        }
        label
    }
}

/// What just happened, in enough detail for the screen to say something useful
/// and for a screen reader to say it out loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// Nothing changed.
    Idle,
    /// A different cell is selected.
    Selected {
        /// The cell now selected.
        index: usize,
    },
    /// A digit was written into a cell.
    Placed {
        /// Where it went.
        index: usize,
        /// What was written.
        digit: u8,
        /// Whether it is the answer's digit.
        correct: bool,
        /// Whether that was the last of this digit on the board.
        completed_digit: bool,
    },
    /// A pencil mark was added or removed.
    Noted {
        /// Which cell.
        index: usize,
        /// Which digit.
        digit: u8,
        /// Whether the mark is now on.
        on: bool,
    },
    /// A cell was cleared.
    Erased {
        /// Which cell.
        index: usize,
    },
    /// The last move was taken back.
    Undone,
    /// The answer filled a cell the player had not.
    Hinted {
        /// Which cell.
        index: usize,
        /// The digit revealed.
        digit: u8,
    },
    /// The puzzle is back to its clues.
    Restarted,
    /// The last digit completed the grid.
    Won,
}

impl Report {
    /// What to say about this report.
    ///
    /// One sentence, written here so the screen and the spoken announcement can
    /// never disagree about what happened. Empty when there is nothing worth
    /// saying.
    #[must_use]
    pub fn words(&self) -> String {
        match *self {
            Self::Idle => String::new(),
            Self::Selected { index } => format!(
                "Row {}, column {}.",
                row_of(index) + 1,
                column_of(index) + 1
            ),
            Self::Placed {
                index,
                digit,
                correct,
                completed_digit,
            } => {
                let place = format!(
                    "{digit} in row {}, column {}.",
                    row_of(index) + 1,
                    column_of(index) + 1
                );
                if !correct {
                    format!("{place} That is not the digit for this cell.")
                } else if completed_digit {
                    format!("{place} That is the last {digit}.")
                } else {
                    place
                }
            }
            Self::Noted { index, digit, on } => {
                let cell = format!("row {}, column {}", row_of(index) + 1, column_of(index) + 1);
                if on {
                    format!("Noted {digit} in {cell}.")
                } else {
                    format!("Removed the note {digit} from {cell}.")
                }
            }
            Self::Erased { index } => {
                format!(
                    "Cleared row {}, column {}.",
                    row_of(index) + 1,
                    column_of(index) + 1
                )
            }
            Self::Undone => "Last move undone.".to_string(),
            Self::Hinted { index, digit } => format!(
                "Row {}, column {} is {digit}.",
                row_of(index) + 1,
                column_of(index) + 1
            ),
            Self::Restarted => "Puzzle restarted.".to_string(),
            Self::Won => "Solved.".to_string(),
        }
    }

    /// The cell this report is about, if any.
    #[must_use]
    pub const fn cell(&self) -> Option<usize> {
        match *self {
            Self::Selected { index }
            | Self::Placed { index, .. }
            | Self::Noted { index, .. }
            | Self::Erased { index }
            | Self::Hinted { index, .. } => Some(index),
            _ => None,
        }
    }
}

/// Everything the screen needs to draw the game's own state.
///
/// Strings rather than numbers because the words are this module's business:
/// "04:12", "no mistakes", "47 left". The screen formats nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readout {
    /// Elapsed time, as a clock.
    pub clock: String,
    /// How many wrong digits have been written.
    pub mistakes: String,
    /// How many cells are still empty.
    pub remaining: String,
    /// What the puzzle is, for the line shown when it is solved.
    pub summary: String,
}

/// A whole game: one puzzle and the player's working of it.
#[derive(Debug, Clone)]
pub struct Game {
    puzzle: Grid,
    solution: Grid,
    values: Grid,
    notes: [Digits; CELLS],
    difficulty: Difficulty,
    selected: Option<usize>,
    notes_mode: bool,
    mistakes: u32,
    hints: u32,
    elapsed: u32,
    running: bool,
    won: bool,
    history: Vec<Snapshot>,
}

/// Everything a move can change, so a move can be taken back exactly.
#[derive(Debug, Clone)]
struct Snapshot {
    values: Grid,
    notes: [Digits; CELLS],
    mistakes: u32,
    hints: u32,
    won: bool,
}

impl Game {
    /// A fresh game of `puzzle`.
    #[must_use]
    pub fn new(puzzle: Grid, solution: Grid, difficulty: Difficulty) -> Self {
        Self {
            puzzle,
            solution,
            values: puzzle,
            notes: [0; CELLS],
            difficulty,
            selected: None,
            notes_mode: false,
            mistakes: 0,
            hints: 0,
            elapsed: 0,
            running: true,
            won: false,
            history: Vec::new(),
        }
    }

    /// A new puzzle of `difficulty`, generated from `seed`.
    #[must_use]
    pub fn start(difficulty: Difficulty, seed: u64) -> Self {
        let puzzle = generate(difficulty, seed);
        Self::new(puzzle.clues, puzzle.solution, difficulty)
    }

    /// The difficulty being played.
    #[must_use]
    pub const fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// The cell the player is working on.
    #[must_use]
    pub const fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Whether the number pad writes pencil marks rather than digits.
    #[must_use]
    pub const fn notes_mode(&self) -> bool {
        self.notes_mode
    }

    /// Whether the grid is finished.
    #[must_use]
    pub const fn is_won(&self) -> bool {
        self.won
    }

    /// Whether there is a move to take back.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// Whether the clock is running.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    /// How many of `digit` are still to be placed, counting only correct ones.
    ///
    /// A wrong digit on the board is not a placed digit: counting it would tell
    /// the player they have finished a digit they have not.
    #[must_use]
    pub fn remaining_of(&self, digit: u8) -> u8 {
        let placed = (0..CELLS)
            .filter(|&index| self.values.get(index) == digit && self.solution.get(index) == digit)
            .count();
        u8::try_from(9 - placed).unwrap_or(0)
    }

    /// All 81 cells, ready to draw.
    #[must_use]
    pub fn cells(&self) -> Vec<Cell> {
        let clashes = self.values.conflicts();
        let selected_digit = self.selected.map_or(0, |index| self.values.get(index));
        let selected_peers = self.selected.map(peers);
        let mut out = Vec::with_capacity(CELLS);
        for (index, &conflict) in clashes.iter().enumerate() {
            let value = self.values.get(index);
            let selected = self.selected == Some(index);
            out.push(Cell {
                index,
                value,
                notes: self.notes[index],
                given: !self.puzzle.is_empty(index),
                wrong: value != 0 && value != self.solution.get(index),
                conflict,
                selected,
                peer: !selected && is_peer(selected_peers.as_ref(), index),
                same: !selected && selected_digit != 0 && value == selected_digit,
            });
        }
        out
    }

    /// Everything the screen says about the game.
    #[must_use]
    pub fn readout(&self) -> Readout {
        let empty = CELLS - self.values.filled();
        let clock = clock_text(self.elapsed);
        let mistakes = self.mistake_count();
        let remaining = match empty {
            0 => "complete".to_string(),
            1 => "1 left".to_string(),
            count => format!("{count} left"),
        };
        let summary = if self.hints == 0 {
            format!("{} · {clock} · {mistakes}", self.difficulty.label())
        } else {
            let plural = if self.hints == 1 { "" } else { "s" };
            format!(
                "{} · {clock} · {mistakes} · {} hint{plural}",
                self.difficulty.label(),
                self.hints
            )
        };
        Readout {
            clock,
            mistakes,
            remaining,
            summary,
        }
    }

    /// The mistake count in words. The one place it becomes a sentence.
    fn mistake_count(&self) -> String {
        match self.mistakes {
            0 => "no mistakes".to_string(),
            1 => "1 mistake".to_string(),
            count => format!("{count} mistakes"),
        }
    }

    /// Select a cell, or clear the selection with `None`.
    ///
    /// Selecting is also how the screen gets its highlight: peers and same-digit
    /// cells are derived in [`Game::cells`].
    pub fn select(&mut self, index: Option<usize>) -> Report {
        if let Some(cell) = index {
            if cell >= CELLS {
                return Report::Idle;
            }
        }
        if self.selected == index {
            return Report::Idle;
        }
        self.selected = index;
        match index {
            Some(cell) => Report::Selected { index: cell },
            None => Report::Idle,
        }
    }

    /// Turn pencil-mark entry on or off.
    pub fn toggle_notes(&mut self) -> Report {
        self.notes_mode = !self.notes_mode;
        Report::Idle
    }

    /// Write `digit` into the selected cell, or toggle it as a pencil mark when
    /// pencil-mark mode is on.
    ///
    /// A clue cell takes neither: the puzzle owns it. Writing a digit that is
    /// not the answer's is allowed and counted — refusing it would turn the game
    /// into a guessing game with a locked keyboard, and the player would learn
    /// nothing from being blocked.
    pub fn enter(&mut self, digit: u8) -> Report {
        if !(1..=9).contains(&digit) {
            return Report::Idle;
        }
        let Some(index) = self.selected else {
            return Report::Idle;
        };
        if !self.puzzle.is_empty(index) {
            return Report::Idle;
        }
        if self.won {
            return Report::Idle;
        }

        if self.notes_mode {
            let already = self.notes[index] & bit(digit) != 0;
            if self.values.get(index) != 0 {
                // Pencil marks live in empty cells. Writing one into a filled
                // cell would show it to nobody and surprise the undo stack.
                return Report::Idle;
            }
            let mut notes = self.notes[index];
            if already {
                notes &= !bit(digit);
            } else {
                notes |= bit(digit);
            }
            self.snapshot();
            self.notes[index] = notes;
            return Report::Noted {
                index,
                digit,
                on: !already,
            };
        }

        let before = self.values.get(index);
        let correct = self.solution.get(index) == digit;
        let completed_digit = correct && self.remaining_of(digit) == 1;
        self.snapshot();
        self.values.set(index, digit);
        self.notes[index] = 0;
        if correct {
            // Only a correct digit is worth trusting enough to strike out of
            // the neighbours' pencil marks. Doing it for a wrong one would erase
            // the player's real deductions on the strength of a typo.
            for peer in peers(index) {
                self.notes[peer] &= !bit(digit);
            }
        } else if before != digit {
            // Re-entering the same wrong digit is not a second mistake: it is
            // the same one. Only a new entry counts.
            self.mistakes = self.mistakes.saturating_add(1);
        }
        self.check_win();
        if self.won {
            return Report::Won;
        }
        Report::Placed {
            index,
            digit,
            correct,
            completed_digit,
        }
    }

    /// Clear the selected cell: its digit, or its pencil marks when they are the
    /// only thing in it.
    pub fn erase(&mut self) -> Report {
        let Some(index) = self.selected else {
            return Report::Idle;
        };
        if !self.puzzle.is_empty(index) || self.values.get(index) == 0 && self.notes[index] == 0 {
            return Report::Idle;
        }
        self.snapshot();
        self.values.set(index, 0);
        self.notes[index] = 0;
        Report::Erased { index }
    }

    /// Take back the last move.
    pub fn undo(&mut self) -> Report {
        let Some(previous) = self.history.pop() else {
            return Report::Idle;
        };
        self.values = previous.values;
        self.notes = previous.notes;
        self.mistakes = previous.mistakes;
        self.hints = previous.hints;
        self.won = previous.won;
        self.running = !self.won;
        Report::Undone
    }

    /// Fill the selected empty cell from the answer, or the first empty cell
    /// when nothing is selected.
    ///
    /// A hint never counts as a mistake, and it clears itself out of the
    /// neighbours' pencil marks like a correct entry does.
    pub fn hint(&mut self) -> Report {
        if self.won {
            return Report::Idle;
        }
        let index = match self.selected {
            Some(index) if self.values.get(index) == 0 => index,
            _ => match (0..CELLS).find(|&index| self.values.get(index) == 0) {
                Some(index) => index,
                None => return Report::Idle,
            },
        };
        let digit = self.solution.get(index);
        if digit == 0 {
            return Report::Idle;
        }
        self.snapshot();
        self.values.set(index, digit);
        self.notes[index] = 0;
        for peer in peers(index) {
            self.notes[peer] &= !bit(digit);
        }
        self.hints = self.hints.saturating_add(1);
        self.selected = Some(index);
        self.check_win();
        Report::Hinted { index, digit }
    }

    /// Put the puzzle back to its clues, keeping the difficulty.
    pub fn restart(&mut self) -> Report {
        self.values = self.puzzle;
        self.notes = [0; CELLS];
        self.mistakes = 0;
        self.hints = 0;
        self.elapsed = 0;
        self.won = false;
        self.running = true;
        self.history.clear();
        Report::Restarted
    }

    /// Advance the clock by one second.
    ///
    /// Does nothing once the puzzle is solved, so a tab left open on the solved
    /// screen does not keep counting.
    pub fn tick(&mut self) {
        if self.running && !self.won {
            self.elapsed = self.elapsed.saturating_add(1);
        }
    }

    /// Stop the clock and show that the grid is finished.
    fn check_win(&mut self) {
        if self.values.is_solved_as(&self.solution) {
            self.won = true;
            self.running = false;
            self.selected = None;
        }
    }

    /// Remember the current board so the next move can be taken back.
    fn snapshot(&mut self) {
        if self.history.len() == HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.history.push(Snapshot {
            values: self.values,
            notes: self.notes,
            mistakes: self.mistakes,
            hints: self.hints,
            won: self.won,
        });
    }

    /// The game as one line, for `localStorage`.
    ///
    /// The answer is not stored: it is recomputed from the clues on load, which
    /// is both smaller and impossible to disagree with.
    #[must_use]
    pub fn to_stored(&self) -> String {
        // Three hex digits per cell: nine bits do not fit in two, and a
        // truncated mask would read back as different pencil marks rather than
        // as an error.
        let notes: String = self
            .notes
            .iter()
            .map(|mask| format!("{mask:0NOTES_WIDTH$x}"))
            .fold(
                String::with_capacity(CELLS * NOTES_WIDTH),
                |mut all, cell| {
                    all.push_str(&cell);
                    all
                },
            );
        format!(
            "{VERSION};{};{};{};{};{};{};{notes}",
            self.difficulty.slug(),
            self.elapsed,
            self.mistakes,
            self.hints,
            self.values.to_line(),
            self.puzzle.to_line(),
        )
    }

    /// A game read back from [`Game::to_stored`]'s line.
    ///
    /// Anything unreadable, truncated, out of range or internally inconsistent
    /// is `None`, and the caller starts a fresh puzzle: a corrupt entry must
    /// never be able to stop the app from starting, and a stored answer is not
    /// trusted when it can be recomputed.
    #[must_use]
    pub fn from_stored(text: &str) -> Option<Self> {
        let mut fields = text.split(';');
        if fields.next()? != VERSION {
            return None;
        }
        let difficulty = Difficulty::from_slug(fields.next()?)?;
        let elapsed = parse_bounded(fields.next()?, 24 * 60 * 60)?;
        let mistakes = parse_bounded(fields.next()?, 9999)?;
        let hints = parse_bounded(fields.next()?, 9999)?;
        let values = Grid::parse(fields.next()?)?;
        let puzzle = Grid::parse(fields.next()?)?;
        let notes_hex = fields.next()?;
        if fields.next().is_some() {
            return None;
        }

        // Clues are the puzzle's; a stored board that disagrees with them is
        // not a game anyone was playing.
        for index in 0..CELLS {
            if !puzzle.is_empty(index) && puzzle.get(index) != values.get(index) {
                return None;
            }
        }
        let solution = solve(&puzzle)?;
        if notes_hex.chars().count() != CELLS * NOTES_WIDTH {
            return None;
        }
        let mut notes = [0; CELLS];
        for (index, slot) in notes.iter_mut().enumerate() {
            let start = index * NOTES_WIDTH;
            let chunk: String = notes_hex.chars().skip(start).take(NOTES_WIDTH).collect();
            let mask = u16::from_str_radix(&chunk, 16).ok()?;
            if mask & !crate::board::ALL_DIGITS != 0 {
                return None;
            }
            *slot = mask;
        }

        let won = values.is_solved_as(&solution);
        Some(Self {
            puzzle,
            solution,
            values,
            notes,
            difficulty,
            selected: None,
            notes_mode: false,
            mistakes,
            hints,
            elapsed,
            running: !won,
            won,
            history: Vec::new(),
        })
    }
}

/// Whether `index` is one of the selected cell's peers.
fn is_peer(selected_peers: Option<&[usize; 20]>, index: usize) -> bool {
    selected_peers.is_some_and(|list| list.contains(&index))
}

/// A stored count, capped, or `None` when it is not a number.
fn parse_bounded(text: &str, cap: u32) -> Option<u32> {
    text.parse::<u32>().ok().map(|value| value.min(cap))
}

/// Seconds as `mm:ss`, or `h:mm:ss` past an hour.
#[must_use]
pub fn clock_text(seconds: u32) -> String {
    let minutes = seconds / 60;
    let rest = seconds % 60;
    if minutes < 60 {
        format!("{minutes:02}:{rest:02}")
    } else {
        format!("{}:{:02}:{rest:02}", minutes / 60, minutes % 60)
    }
}

/// The digits a cell's pencil marks currently hold, ascending.
pub fn note_digits(mask: Digits) -> impl Iterator<Item = u8> {
    digits_of(mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::solve;

    /// A small puzzle with a known answer, used by most tests here.
    const PUZZLE: &str = "53..7....\
                          6..195...\
                          .98....6.\
                          8...6...3\
                          4..8.3..1\
                          7...2...6\
                          .6....28.\
                          ...419..5\
                          ....8..79";

    fn game() -> Game {
        let clues = Grid::parse(PUZZLE).expect("a parseable puzzle");
        let solution: Grid = solve(&clues).expect("a solvable puzzle");
        Game::new(clues, solution, Difficulty::Medium)
    }

    /// The first empty peer of `index`, which is where a pencil-mark test has
    /// to work: the app refuses to mark a clue cell, so a filled neighbour
    /// would test nothing.
    fn empty_peer(game: &Game, index: usize) -> usize {
        peers(index)
            .into_iter()
            .find(|&peer| game.values.is_empty(peer))
            .expect("an empty peer")
    }

    /// The first cell that is empty, in reading order.
    fn first_empty(game: &Game) -> usize {
        game.cells()
            .into_iter()
            .find(|cell| cell.value == 0)
            .expect("an empty cell")
            .index
    }

    #[test]
    fn a_new_game_starts_on_its_clues_with_a_clean_clock() {
        let game = game();
        assert_eq!(
            game.values.filled(),
            Grid::parse(PUZZLE).expect("a parseable puzzle").filled()
        );
        assert_eq!(game.cells().len(), CELLS);
        assert!(!game.is_won());
        assert!(game.is_running());
        assert_eq!(game.selected(), None);
        assert_eq!(game.readout().clock, "00:00");
        assert_eq!(game.readout().mistakes, "no mistakes");
        assert_eq!(game.readout().remaining, "51 left");
    }

    #[test]
    fn a_selected_cell_shows_its_peers_and_its_digit_twins() {
        let mut game = game();
        // Row 1, column 1 holds a 5; row 1, column 2 is empty.
        game.select(Some(1));
        let cells = game.cells();
        assert!(cells[1].selected);
        assert!(cells[0].peer, "same row");
        assert!(cells[9].peer, "same column");
        assert!(cells[10].peer, "same box");
        assert!(!cells[80].peer, "unrelated");
        assert!(cells[0].given);
        assert!(!cells[0].wrong);
    }

    #[test]
    fn a_clue_cell_cannot_be_written_over() {
        let mut game = game();
        game.select(Some(0));
        assert_eq!(game.enter(4), Report::Idle);
        assert_eq!(game.cells()[0].value, 5);
    }

    #[test]
    fn a_correct_digit_is_accepted_and_clears_the_pencil_marks_around_it() {
        let mut game = game();
        let index = first_empty(&game);
        // Pencil-mark the answer into a neighbour, then place it for real.
        let digit = game.solution.get(index);
        let neighbour = empty_peer(&game, index);
        game.select(Some(neighbour));
        game.toggle_notes();
        game.enter(digit);
        assert!(game.cells()[neighbour].notes & bit(digit) != 0);

        game.toggle_notes();
        game.select(Some(index));
        let report = game.enter(digit);
        assert!(
            matches!(report, Report::Placed { correct: true, .. }),
            "{report:?}"
        );
        assert_eq!(game.cells()[index].value, digit);
        assert_eq!(
            game.cells()[neighbour].notes & bit(digit),
            0,
            "a placed digit is struck out of the neighbours' pencil marks"
        );
        assert_eq!(game.readout().mistakes, "no mistakes");
    }

    #[test]
    fn a_wrong_digit_is_kept_visible_and_counted_once() {
        let mut game = game();
        let index = first_empty(&game);
        let answer = game.solution.get(index);
        let wrong = if answer == 9 { 1 } else { answer + 1 };
        game.select(Some(index));

        let report = game.enter(wrong);
        assert!(matches!(report, Report::Placed { correct: false, .. }));
        assert!(game.cells()[index].wrong);
        assert_eq!(game.readout().mistakes, "1 mistake");
        // The same wrong digit again is the same mistake, not a second one.
        game.enter(wrong);
        assert_eq!(game.readout().mistakes, "1 mistake");
        // A different wrong digit is a new attempt.
        let other = if wrong == 9 { 1 } else { wrong + 1 };
        let other = if other == answer {
            if other == 9 {
                1
            } else {
                other + 1
            }
        } else {
            other
        };
        game.enter(other);
        assert_eq!(game.readout().mistakes, "2 mistakes");
    }

    #[test]
    fn pencil_marks_toggle_on_and_off_and_never_overwrite_a_digit() {
        let mut game = game();
        let index = first_empty(&game);
        game.select(Some(index));
        game.toggle_notes();
        assert!(matches!(game.enter(4), Report::Noted { on: true, .. }));
        assert_eq!(game.cells()[index].notes, bit(4));
        assert!(matches!(game.enter(4), Report::Noted { on: false, .. }));
        assert_eq!(game.cells()[index].notes, 0);
        game.toggle_notes();

        // A filled cell refuses pencil marks.
        let digit = game.solution.get(index);
        game.enter(digit);
        game.toggle_notes();
        assert_eq!(game.enter(7), Report::Idle);
        assert_eq!(game.cells()[index].value, digit);
    }

    /// Give a cell a couple of pencil marks, leaving pencil-mark mode off
    /// again, so the tests about clearing have something to clear.
    fn note_two_digits(game: &mut Game, index: usize) {
        game.toggle_notes();
        game.select(Some(index));
        game.enter(2);
        game.enter(3);
        game.toggle_notes();
    }

    #[test]
    fn erasing_clears_a_digit_but_never_a_clue() {
        let mut game = game();
        let index = first_empty(&game);
        let digit = game.solution.get(index);
        game.select(Some(index));
        game.enter(digit);
        assert!(matches!(game.erase(), Report::Erased { .. }));
        assert_eq!(game.cells()[index].value, 0);

        game.select(Some(0));
        assert_eq!(game.erase(), Report::Idle);
        assert_eq!(game.cells()[0].value, 5);
    }

    #[test]
    fn undo_puts_back_the_digit_the_pencil_marks_and_the_mistake_count() {
        let mut game = game();
        let index = first_empty(&game);
        let answer = game.solution.get(index);
        let wrong = if answer == 9 { 1 } else { answer + 1 };
        // The clean readout is the one to come back to. Comparing against a
        // readout taken *after* the mistake would pass only if undo did
        // nothing at all.
        let clean = game.readout();

        game.select(Some(index));
        game.enter(wrong);
        game.erase();

        // Two moves back: the erase, then the wrong digit. The mistake count
        // has to come back with the digit, not survive it.
        assert_eq!(game.undo(), Report::Undone);
        assert_eq!(game.cells()[index].value, wrong, "the digit comes back");
        assert_eq!(game.readout().mistakes, "1 mistake");
        assert_eq!(game.undo(), Report::Undone);
        assert_eq!(game.cells()[index].value, 0);
        assert_eq!(game.readout(), clean, "undo restores the whole readout");
        assert_eq!(game.undo(), Report::Idle, "nothing left to undo");
    }

    #[test]
    fn undo_restores_notes_a_placement_had_struck_out() {
        let mut game = game();
        let index = first_empty(&game);
        let digit = game.solution.get(index);
        let neighbour = empty_peer(&game, index);
        note_two_digits(&mut game, neighbour);
        // Make sure the neighbour's marks include the digit about to be placed.
        game.toggle_notes();
        game.select(Some(neighbour));
        game.enter(digit);
        game.toggle_notes();
        let marks = game.cells()[neighbour].notes;
        assert!(marks & bit(digit) != 0);

        game.select(Some(index));
        game.enter(digit);
        assert_eq!(game.cells()[neighbour].notes & bit(digit), 0);
        game.undo();
        assert_eq!(
            game.cells()[neighbour].notes,
            marks,
            "undo puts the struck-out pencil marks back"
        );
    }

    #[test]
    fn the_history_is_bounded() {
        let mut game = game();
        let index = first_empty(&game);
        game.select(Some(index));
        game.toggle_notes();
        for digit in 1..=9u8 {
            for _ in 0..HISTORY_LIMIT {
                game.enter(digit);
            }
        }
        assert_eq!(game.history.len(), HISTORY_LIMIT);
    }

    #[test]
    fn a_hint_fills_the_answer_and_is_never_a_mistake() {
        let mut game = game();
        let index = first_empty(&game);
        game.select(Some(index));
        let digit = game.solution.get(index);
        assert_eq!(game.hint(), Report::Hinted { index, digit });
        assert_eq!(game.cells()[index].value, digit);
        assert!(!game.cells()[index].wrong);
        assert_eq!(game.readout().mistakes, "no mistakes");
        assert!(game.readout().summary.contains("1 hint"));
    }

    #[test]
    fn the_clock_runs_until_the_grid_is_solved() {
        let mut game = game();
        for _ in 0..61 {
            game.tick();
        }
        assert_eq!(game.readout().clock, "01:01");
        while !game.is_won() {
            game.hint();
        }
        assert!(!game.is_running());
        let stopped = game.readout().clock;
        game.tick();
        assert_eq!(game.readout().clock, stopped);
        assert_eq!(game.readout().remaining, "complete");
        assert_eq!(
            game.readout().summary,
            format!("Medium · {stopped} · no mistakes · 51 hints")
        );
    }

    #[test]
    fn solving_the_last_cell_reports_a_win() {
        let mut game = game();
        // Fill every cell but one from the answer, as a hint would.
        let mut last = None;
        for index in 0..CELLS {
            if game.values.is_empty(index) {
                if last.is_some() {
                    game.select(Some(index));
                    game.hint();
                } else {
                    last = Some(index);
                }
            }
        }
        let last = last.expect("a cell");
        assert!(!game.is_won());
        game.select(Some(last));
        assert_eq!(game.enter(game.solution.get(last)), Report::Won);
        assert!(game.is_won());
        assert!(!game.is_running());
        assert_eq!(game.selected(), None);
    }

    #[test]
    fn restarting_returns_the_puzzle_to_its_clues() {
        let mut game = game();
        let index = first_empty(&game);
        game.select(Some(index));
        game.enter(game.solution.get(index));
        game.tick();
        assert_eq!(game.restart(), Report::Restarted);
        assert_eq!(game.cells()[index].value, 0);
        assert_eq!(game.readout().clock, "00:00");
        assert_eq!(game.readout().mistakes, "no mistakes");
        assert_eq!(game.undo(), Report::Idle, "a restart cannot be undone");
    }

    #[test]
    fn remaining_counts_only_digits_in_their_place() {
        let mut game = game();
        let index = first_empty(&game);
        let answer = game.solution.get(index);
        let wrong = if answer == 9 { 1 } else { answer + 1 };
        // Counted before the board changes, so the borrow of the puzzle is not
        // held across a mutation.
        let clues_of = |digit: u8| {
            (0..CELLS)
                .filter(|&cell| game.puzzle.get(cell) == digit)
                .count()
        };
        let answer_clues = clues_of(answer);
        let wrong_clues = clues_of(wrong);
        assert_eq!(
            usize::from(game.remaining_of(answer)),
            9 - answer_clues,
            "a square's own digit must be counted as placed"
        );

        // A wrong digit is not a placed one: it must not look like one.
        game.select(Some(index));
        game.enter(wrong);
        assert_eq!(
            usize::from(game.remaining_of(wrong)),
            9 - wrong_clues,
            "a misplaced digit must not count as placed"
        );
    }

    #[test]
    fn a_game_round_trips_through_storage() {
        let mut game = game();
        let index = first_empty(&game);
        game.select(Some(index));
        game.enter(game.solution.get(index));
        game.tick();
        let neighbour = peers(index)[1];
        game.select(Some(neighbour));
        game.toggle_notes();
        game.enter(3);
        game.enter(8);

        let stored = game.to_stored();
        let back = Game::from_stored(&stored).expect("a readable game");
        assert_eq!(back.to_stored(), stored);
        assert_eq!(back.difficulty(), Difficulty::Medium);
        assert_eq!(back.cells()[index].value, game.cells()[index].value);
        assert_eq!(back.cells()[neighbour].notes, game.cells()[neighbour].notes);
        assert_eq!(back.readout(), game.readout());
        assert!(!back.is_won());
    }

    #[test]
    fn a_solved_game_survives_storage_as_solved() {
        let mut game = game();
        while !game.is_won() {
            game.hint();
        }
        let back = Game::from_stored(&game.to_stored()).expect("a readable game");
        assert!(back.is_won());
        assert!(!back.is_running());
    }

    #[test]
    fn corrupt_storage_is_refused_rather_than_half_read() {
        let good = game().to_stored();
        let mut broken = vec![
            String::new(),
            "v2;easy;0;0;0;".to_string() + &good,
            good.replace("v1;medium", "v1;impossible"),
            good.replace(";medium;", ";medium;x"),
            // A board that disagrees with its own clues.
            good.clone(),
        ];
        let short = &good[..good.len() - 10];
        broken.push(short.to_string());
        let mut missing = good.split(';').collect::<Vec<_>>();
        missing.pop();
        broken.push(missing.join(";"));

        for text in broken {
            let parsed = Game::from_stored(&text);
            if text == good {
                // The untouched copy is the control: it must parse.
                assert!(parsed.is_some(), "the control line must remain readable");
            } else {
                assert!(parsed.is_none(), "accepted a broken line: {text}");
            }
        }
    }

    #[test]
    fn the_stored_line_is_the_documented_width() {
        let stored = game().to_stored();
        let fields: Vec<&str> = stored.split(';').collect();
        assert_eq!(fields.len(), 8);
        assert_eq!(fields[0], "v1");
        assert_eq!(fields[1], "medium");
        assert_eq!(fields[5].len(), CELLS);
        assert_eq!(fields[6].len(), CELLS);
        assert_eq!(fields[7].len(), CELLS * NOTES_WIDTH);
    }

    #[test]
    fn the_clock_text_is_a_clock() {
        assert_eq!(clock_text(0), "00:00");
        assert_eq!(clock_text(9), "00:09");
        assert_eq!(clock_text(61), "01:01");
        assert_eq!(clock_text(3599), "59:59");
        assert_eq!(clock_text(3600), "1:00:00");
        assert_eq!(clock_text(7322), "2:02:02");
    }

    #[test]
    fn reports_say_what_happened_without_naming_how_the_app_is_built() {
        let index = 10;
        let words = Report::Placed {
            index,
            digit: 4,
            correct: true,
            completed_digit: false,
        }
        .words();
        assert_eq!(words, "4 in row 2, column 2.");
        assert!(words.to_lowercase().contains("row"));
        for report in [
            Report::Undone,
            Report::Restarted,
            Report::Won,
            Report::Selected { index },
            Report::Hinted { index, digit: 4 },
            Report::Erased { index },
            Report::Noted {
                index,
                digit: 4,
                on: true,
            },
            Report::Noted {
                index,
                digit: 4,
                on: false,
            },
        ] {
            let words = report.words().to_lowercase();
            assert!(!words.is_empty(), "{report:?} has nothing to say");
            for forbidden in ["rust", "wasm", "webassembly", "javascript", "bindgen"] {
                assert!(
                    !words.contains(forbidden),
                    "{report:?} names the implementation"
                );
            }
        }
        assert_eq!(Report::Idle.words(), "");
    }
}
