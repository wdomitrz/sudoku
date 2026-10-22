//! The puzzle: the grid, the rules, a solver, and a generator.
//!
//! No `web-sys`, no `js-sys`, no `wasm-bindgen`, so this module compiles and
//! runs natively under `cargo test`. Everything a person can reason about —
//! which digit is allowed where, whether a puzzle has one answer, how hard it
//! is — is decided here and tested here.
//!
//! The generator is seeded rather than random: the same seed always produces
//! the same puzzle, which is what makes generation testable at all. The seed
//! itself is the caller's business; `crate::ui` draws it from the browser.

/// The side of the grid.
pub const SIZE: usize = 9;

/// The number of cells in a grid, in reading order.
pub const CELLS: usize = SIZE * SIZE;

/// A set of digits, as a bit mask: bit `d` is set when digit `d` is in the set.
///
/// Nine digits fit in nine bits with room to spare, which makes candidate sets,
/// "which digits are missing from this row" and pencil marks all the same cheap
/// value type instead of three.
pub type Digits = u16;

/// The mask with only `digit` set.
///
/// Digit `0` is not a digit — it is an empty cell — so this returns `0` for it
/// rather than quietly setting bit zero, which is what lets a single fold over
/// a grid's cells answer "which digits are present here" without filtering.
#[must_use]
pub const fn bit(digit: u8) -> Digits {
    if digit == 0 || digit > 9 {
        0
    } else {
        1u16 << digit
    }
}

/// Every digit, `1..=9`.
pub const ALL_DIGITS: Digits = 0b0000_0011_1111_1110;

/// The digits in `mask`, ascending.
pub fn digits_of(mask: Digits) -> impl Iterator<Item = u8> {
    (1..=9u8).filter(move |&digit| mask & bit(digit) != 0)
}

/// How many digits are in `mask`.
#[must_use]
pub fn count_of(mask: Digits) -> u32 {
    mask.count_ones()
}

/// A 9×9 grid of digits, where `0` means empty.
///
/// The value is the whole board state: no hidden invariants, no partial
/// updates. An invalid grid can exist — a partly filled puzzle is one — so
/// validity is asked for ([`Grid::is_valid`], [`Grid::conflicts`]) rather than
/// promised by the type.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    cells: [u8; CELLS],
}

impl Default for Grid {
    /// An empty grid. Written out rather than derived: arrays only derive
    /// `Default` up to 32 elements, and a 9×9 board is 81.
    fn default() -> Self {
        Self::empty()
    }
}

impl Grid {
    /// An empty grid.
    #[must_use]
    pub const fn empty() -> Self {
        Self { cells: [0; CELLS] }
    }

    /// A grid from cells in reading order, or `None` if any value is not a
    /// digit and not empty.
    #[must_use]
    pub fn from_cells(cells: [u8; CELLS]) -> Option<Self> {
        if cells.iter().any(|&value| value > 9) {
            return None;
        }
        Some(Self { cells })
    }

    /// A grid parsed from 81 characters: `1`–`9` for a digit, and `.` or `0`
    /// for an empty cell.
    ///
    /// Anything else, including a string of the wrong length, is `None`. This
    /// is the form used by tests and by the puzzles a reader pastes in; the
    /// stored game uses [`Grid::to_line`]'s digits-only form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut cells = [0u8; CELLS];
        let mut chars = text.chars().filter(|character| !character.is_whitespace());
        for slot in &mut cells {
            let character = chars.next()?;
            *slot = match character {
                '.' | '0' => 0,
                '1'..='9' => u8::try_from(character).ok()? - b'0',
                _ => return None,
            };
        }
        if chars.next().is_some() {
            return None;
        }
        Some(Self { cells })
    }

    /// The grid as 81 digits, `0` for an empty cell.
    ///
    /// The form the game is stored in: fixed width, no separators, and
    /// impossible to read back as anything but a grid.
    #[must_use]
    pub fn to_line(&self) -> String {
        self.cells
            .iter()
            .map(|&value| char::from(b'0' + value))
            .collect()
    }

    /// The digit at `index`, or `0` when the cell is empty.
    ///
    /// # Panics
    ///
    /// Panics when `index` is not a cell of the grid. Callers hold cells that
    /// came from [`CELLS`]-bounded loops, so this is a programming error, not
    /// an input error.
    #[must_use]
    pub fn get(&self, index: usize) -> u8 {
        self.cells[index]
    }

    /// Place `digit` at `index`. `0` clears the cell.
    ///
    /// # Panics
    ///
    /// Panics when `index` is not a cell of the grid, or when `digit` is above
    /// 9.
    pub fn set(&mut self, index: usize, digit: u8) {
        assert!(digit <= 9, "digit {digit} is not a digit");
        self.cells[index] = digit;
    }

    /// Whether the cell at `index` is empty.
    ///
    /// # Panics
    ///
    /// Panics when `index` is not a cell of the grid.
    #[must_use]
    pub fn is_empty(&self, index: usize) -> bool {
        self.cells[index] == 0
    }

    /// How many cells hold a digit.
    #[must_use]
    pub fn filled(&self) -> usize {
        self.cells.iter().filter(|&&value| value != 0).count()
    }

    /// The digits placed anywhere in the grid, as a mask.
    #[must_use]
    pub fn present(&self) -> Digits {
        self.cells.iter().fold(0, |mask, &value| mask | bit(value))
    }

    /// The digits this cell could still take, without breaking a row, column or
    /// box. An occupied cell has no candidates.
    ///
    /// # Panics
    ///
    /// Panics when `index` is not a cell of the grid.
    #[must_use]
    pub fn candidates(&self, index: usize) -> Digits {
        if !self.is_empty(index) {
            return 0;
        }
        let mut taken = 0;
        for peer in peers(index) {
            taken |= bit(self.cells[peer]);
        }
        ALL_DIGITS & !taken
    }

    /// Every cell that repeats a digit in its row, column or box.
    ///
    /// Both sides of a clash are flagged — the digit just typed and the one it
    /// collides with — because a reader looking at a red cell needs to see what
    /// it disagrees with.
    #[must_use]
    pub fn conflicts(&self) -> [bool; CELLS] {
        let mut flagged = [false; CELLS];
        for index in 0..CELLS {
            let digit = self.cells[index];
            if digit == 0 {
                continue;
            }
            for peer in peers(index) {
                if self.cells[peer] == digit {
                    flagged[index] = true;
                    flagged[peer] = true;
                }
            }
        }
        flagged
    }

    /// Whether no digit repeats in any row, column or box.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.conflicts().iter().any(|&clash| clash)
    }

    /// Whether every cell holds a digit.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.cells.iter().all(|&value| value != 0)
    }

    /// Whether every cell holds the same digit as `other`. An empty grid is
    /// never a solution.
    #[must_use]
    pub fn is_solved_as(&self, other: &Self) -> bool {
        self.is_complete() && self == other
    }
}

impl std::fmt::Debug for Grid {
    /// Nine lines, which is what a failing assertion needs to be readable.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter)?;
        for row in 0..SIZE {
            for column in 0..SIZE {
                let digit = self.cells[row * SIZE + column];
                if digit == 0 {
                    write!(formatter, ".")?;
                } else {
                    write!(formatter, "{digit}")?;
                }
                if column == 2 || column == 5 {
                    write!(formatter, " ")?;
                }
            }
            if row == 2 || row == 5 {
                writeln!(formatter)?;
            }
            writeln!(formatter)?;
        }
        Ok(())
    }
}

/// The row, column and box of a cell.
///
/// Integer division only, so any `usize` is a cell number some grid would have;
/// only the caller knows whether this grid has it.
#[must_use]
pub fn row_of(index: usize) -> usize {
    index / SIZE
}

/// The column of a cell.
#[must_use]
pub fn column_of(index: usize) -> usize {
    index % SIZE
}

/// The 3×3 box of a cell, numbered left to right and then top to bottom.
#[must_use]
pub fn box_of(index: usize) -> usize {
    (row_of(index) / 3) * 3 + column_of(index) / 3
}

/// The 20 cells a cell shares a row, column or box with.
///
/// `20 = 8 + 8 + 4`: the peers of the row and column that are not already in
/// the row and column, plus the rest of the box. Written as an array rather
/// than an iterator so the length is a compile-time fact — every candidate
/// computation and every pencil-mark update depends on it being complete.
///
/// # Panics
///
/// Panics when `index` is not a cell of the grid.
#[must_use]
pub fn peers(index: usize) -> [usize; 20] {
    let row = row_of(index);
    let column = column_of(index);
    let mut out = [0usize; 20];
    let mut at = 0;
    for other in 0..SIZE {
        if other != column {
            out[at] = row * SIZE + other;
            at += 1;
        }
    }
    for other in 0..SIZE {
        if other != row {
            out[at] = other * SIZE + column;
            at += 1;
        }
    }
    let box_row = (row / 3) * 3;
    let box_column = (column / 3) * 3;
    for other_row in box_row..box_row + 3 {
        for other_column in box_column..box_column + 3 {
            if other_row != row && other_column != column {
                out[at] = other_row * SIZE + other_column;
                at += 1;
            }
        }
    }
    out
}

/// A small, fast, reproducible generator of random numbers.
///
/// Not a cryptography-grade source and not trying to be: it exists so that a
/// seed determines a puzzle. `xorshift64*` is the whole algorithm.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator starting from `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        // Zero is a fixed point of xorshift, so a zero seed would repeat zero
        // for ever; any other seed is fine.
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    /// The next value.
    pub fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A value in `0..bound`, or `0` when `bound` is zero.
    ///
    /// Scaling rather than rejection: the bias is irrelevant to shuffling a
    /// sudoku and rejection would risk an unbounded loop.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        let wide = self.next_u64() >> 32;
        let limit = u64::try_from(bound).unwrap_or(u64::MAX);
        usize::try_from(wide % limit).unwrap_or(0)
    }

    /// Shuffle `items` in place (Fisher–Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for index in (1..items.len()).rev() {
            let other = self.below(index + 1);
            items.swap(index, other);
        }
    }
}

/// How hard a puzzle is meant to be.
///
/// Difficulty is a target number of clues, and that is all: a puzzle with fewer
/// clues is a puzzle with more cells to deduce. It is an honest, checkable
/// definition — every generated puzzle in [`Difficulty::ALL`] is proven to have
/// exactly one answer — and it is not a claim about which techniques a solver
/// needs. A human may find a 26-clue puzzle easy if the deductions cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Difficulty {
    /// Room to think: forty clues.
    Easy,
    /// The usual newspaper puzzle: thirty-two clues.
    Medium,
    /// Few clues, and no guessing required: twenty-six.
    Hard,
}

impl Difficulty {
    /// Every difficulty, easiest first.
    pub const ALL: [Self; 3] = [Self::Easy, Self::Medium, Self::Hard];

    /// How many clues this difficulty aims for.
    #[must_use]
    pub const fn clues(self) -> usize {
        match self {
            Self::Easy => 40,
            Self::Medium => 32,
            Self::Hard => 26,
        }
    }

    /// The word for this difficulty.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }

    /// The difficulty's name as stored and parsed, so the saved game survives a
    /// change to the visible wording.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
        }
    }

    /// The difficulty a stored slug names, or `None`.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|entry| entry.slug() == slug)
    }

    /// The next difficulty, wrapping round.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Easy => Self::Medium,
            Self::Medium => Self::Hard,
            Self::Hard => Self::Easy,
        }
    }
}

/// A puzzle and its one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Puzzle {
    /// The clue cells. Every other cell is `0`.
    pub clues: Grid,
    /// The completed grid, and the only completion of `clues`.
    pub solution: Grid,
}

/// The search budget, in nodes, for one proof of uniqueness.
///
/// A generated puzzle's uniqueness is checked up to almost a hundred times
/// (once per attempted clue removal), so anything that can blow up here is felt
/// as a frozen tab. Real puzzles are proven in a few thousand nodes; this is far
/// above that, and [`count_solutions`] treats exhaustion as "not proven unique",
/// which keeps the clue rather than claiming an answer that was never checked.
const NODE_BUDGET: u32 = 200_000;

/// Solve a grid: one complete grid that agrees with every clue, if there is one.
#[must_use]
pub fn solve(grid: &Grid) -> Option<Grid> {
    let mut search = Search::new(grid)?;
    search.run(None, 1);
    search.first
}

/// Count the solutions to a grid, stopping at `limit`.
///
/// Returns at most `limit`. A search that exhausts the node budget without
/// settling the count reports `limit`, so a caller asking "is this unique?"
/// (`limit = 2`) is never told a puzzle is unique on the strength of an
/// unfinished proof.
#[must_use]
pub fn count_solutions(grid: &Grid, limit: u8) -> u8 {
    match Search::new(grid) {
        Some(mut search) => {
            search.run(None, limit);
            search.found.min(limit)
        }
        // A clue that already repeats a digit has no completion at all.
        None => 0,
    }
}

/// Generate a puzzle with exactly one answer, aiming for `difficulty`'s clue
/// count.
///
/// The same seed and difficulty always produce the same puzzle. The clue count
/// is a target rather than a promise: clues are dropped one at a time in random
/// order, and one whose removal would leave a second answer is put back. So a
/// puzzle ends with the target's clues when the order allows it, and with a few
/// more when it does not — never with a second answer.
#[must_use]
pub fn generate(difficulty: Difficulty, seed: u64) -> Puzzle {
    let mut rng = Rng::new(seed);

    // A full grid, by randomized search: every placement is tried in a random
    // order, so the answer is a different valid grid for almost every seed.
    let empty = Grid::empty();
    let Some(mut search) = Search::new(&empty) else {
        // Unreachable: an empty grid has no contradiction to find. Returning an
        // empty puzzle rather than panicking keeps generation total, which
        // matters because this runs on a tap.
        return Puzzle {
            clues: empty,
            solution: empty,
        };
    };
    search.run(Some(&mut rng), 1);
    let solution = search.first.unwrap_or(empty);

    // Dig clues out, in random order, keeping only those whose removal leaves
    // one answer.
    let mut clues = solution;
    let mut order: Vec<usize> = (0..CELLS).collect();
    rng.shuffle(&mut order);
    for index in order {
        if clues.filled() <= difficulty.clues() {
            break;
        }
        let held = clues.get(index);
        clues.set(index, 0);
        if count_solutions(&clues, 2) != 1 {
            clues.set(index, held);
        }
    }

    Puzzle { clues, solution }
}

/// A depth-first search over the grid, keeping candidate sets up to date.
///
/// The state is the grid plus one candidate mask per cell. Every cell's mask is
/// recomputed from the placed digits when it changes, which is more work per
/// node than a trail of incremental updates and much harder to get wrong.
struct Search {
    cells: [u8; CELLS],
    candidates: [Digits; CELLS],
    found: u8,
    limit: u8,
    nodes: u32,
    first: Option<Grid>,
}

impl Search {
    /// A search over `grid`, or `None` when the clues already contradict each
    /// other.
    fn new(grid: &Grid) -> Option<Self> {
        // The clues have to agree with each other before any search begins.
        // Without this, two 5s in a row are simply never looked at — they are
        // not empty cells, so no candidate mask covers them — and the search
        // returns a "solution" containing both.
        if !grid.is_valid() {
            return None;
        }
        let mut search = Self {
            cells: grid.cells,
            candidates: [0; CELLS],
            found: 0,
            limit: 1,
            nodes: 0,
            first: None,
        };
        if search.refresh() {
            Some(search)
        } else {
            None
        }
    }

    /// Recompute every candidate mask. `false` when a cell has no candidates
    /// left, which means this branch is dead.
    fn refresh(&mut self) -> bool {
        let snapshot = Grid { cells: self.cells };
        for index in 0..CELLS {
            self.candidates[index] = snapshot.candidates(index);
            if self.cells[index] == 0 && self.candidates[index] == 0 {
                return false;
            }
        }
        true
    }

    /// Search until `limit` answers are found or the budget runs out.
    fn run(&mut self, mut rng: Option<&mut Rng>, limit: u8) {
        self.limit = limit.max(1);
        self.find(&mut rng);
    }

    /// One level of the search, recursing on the most constrained cell.
    fn find(&mut self, rng: &mut Option<&mut Rng>) {
        if self.nodes >= NODE_BUDGET {
            // Out of budget. Report the full count so a uniqueness check fails
            // closed rather than claiming an answer nobody proved.
            self.found = self.limit;
            return;
        }
        self.nodes += 1;

        // The most constrained empty cell first: it keeps the tree small
        // enough that digging a puzzle out stays interactive.
        let mut choice: Option<(usize, Digits)> = None;
        for index in 0..CELLS {
            if self.cells[index] != 0 {
                continue;
            }
            let mask = self.candidates[index];
            if mask == 0 {
                return;
            }
            match choice {
                Some((_, best)) if count_of(best) <= count_of(mask) => {}
                _ => choice = Some((index, mask)),
            }
        }

        let Some((index, mask)) = choice else {
            // No empty cell: this is an answer.
            self.found += 1;
            if self.first.is_none() {
                self.first = Some(Grid { cells: self.cells });
            }
            return;
        };

        let mut order: Vec<u8> = digits_of(mask).collect();
        if let Some(generator) = rng {
            generator.shuffle(&mut order);
        }
        for digit in order {
            self.cells[index] = digit;
            let consistent = self.refresh();
            if consistent {
                self.find(rng);
            }
            self.cells[index] = 0;
            if self.found >= self.limit {
                // Leave the board as it was found, so a caller that abandons a
                // search mid-tree still holds a sound grid.
                let _ = self.refresh();
                return;
            }
        }
        let _ = self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The puzzle the generator tests compare against: a real, hard, one-answer
    /// grid.
    const HARD: &str = "8........\
                        ..36.....\
                        .7..9.2..\
                        .5...7...\
                        ....457..\
                        ...1...3.\
                        ..1....68\
                        ..85...1.\
                        .9....4..";

    fn grid(text: &str) -> Grid {
        Grid::parse(text).expect("a parseable grid")
    }

    #[test]
    fn a_grid_round_trips_through_both_text_forms() {
        let parsed = grid(HARD);
        assert_eq!(parsed.to_line().len(), CELLS);
        assert_eq!(Grid::parse(&parsed.to_line()), Some(parsed));
        assert_eq!(parsed.filled(), 21);
    }

    #[test]
    fn malformed_text_is_refused_rather_than_guessed() {
        assert_eq!(Grid::parse("123"), None);
        assert_eq!(Grid::parse(&"1".repeat(82)), None);
        assert_eq!(Grid::parse(&format!("{}x", "1".repeat(80))), None);
        // Whitespace is layout, not content: the multi-line form parses.
        assert!(Grid::parse(&format!("{}\n{}\n", "1".repeat(40), "2".repeat(41))).is_some());
        // And a value outside 1..=9 is refused by the array form.
        let mut cells = [0u8; CELLS];
        cells[0] = 10;
        assert_eq!(Grid::from_cells(cells), None);
    }

    #[test]
    fn the_puzzle_solves_to_a_complete_valid_grid() {
        let answer = solve(&grid(HARD)).expect("one answer");
        assert!(answer.is_complete());
        assert!(answer.is_valid());
        // Every clue survived.
        let clues = grid(HARD);
        for index in 0..CELLS {
            if !clues.is_empty(index) {
                assert_eq!(answer.get(index), clues.get(index), "clue {index}");
            }
        }
    }

    #[test]
    fn a_contradiction_has_no_answer_and_no_solutions() {
        // Two 5s in the first row.
        let mut broken = Grid::empty();
        broken.set(0, 5);
        broken.set(1, 5);
        assert!(broken.conflicts()[0] && broken.conflicts()[1]);
        assert_eq!(solve(&broken), None);
        assert_eq!(count_solutions(&broken, 2), 0);
    }

    #[test]
    fn uniqueness_is_what_the_counter_reports() {
        // One answer.
        assert_eq!(count_solutions(&grid(HARD), 2), 1);
        // An empty grid has many, and the count stops at the limit.
        assert_eq!(count_solutions(&Grid::empty(), 2), 2);
        // A solved grid has exactly itself.
        let answer = solve(&grid(HARD)).expect("one answer");
        assert_eq!(count_solutions(&answer, 2), 1);
    }

    #[test]
    fn candidates_exclude_every_placed_peer() {
        let mut one_cell = Grid::empty();
        // Surround the top-left cell with a full row, column and box.
        for column in 1..9 {
            one_cell.set(column, u8::try_from(column).expect("a digit"));
        }
        one_cell.set(9, 4);
        one_cell.set(10, 7);
        one_cell.set(1, 0);
        let candidates = one_cell.candidates(0);
        // The row and box between them place 2-8, so only 1 and 9 are left.
        assert_eq!(digits_of(candidates).collect::<Vec<_>>(), vec![1, 9]);
        // An occupied cell has none.
        assert_eq!(one_cell.candidates(2), 0);
    }

    #[test]
    fn peers_are_the_twenty_cells_that_share_a_row_column_or_box() {
        for index in 0..CELLS {
            let list = peers(index);
            assert!(!list.contains(&index), "cell {index} is not its own peer");
            let mut unique = list.to_vec();
            unique.sort_unstable();
            let before = unique.len();
            unique.dedup();
            assert_eq!(unique.len(), before, "cell {index} repeats a peer");
            for peer in list {
                let shares_row_or_column =
                    row_of(peer) == row_of(index) || column_of(peer) == column_of(index);
                assert!(
                    shares_row_or_column || box_of(peer) == box_of(index),
                    "cell {index} is not related to {peer}"
                );
            }
        }
    }

    #[test]
    fn a_generated_puzzle_has_exactly_one_answer_and_it_is_the_one_recorded() {
        for difficulty in Difficulty::ALL {
            for seed in 1..=4u64 {
                let puzzle = generate(difficulty, seed);
                assert!(
                    puzzle.solution.is_valid() && puzzle.solution.is_complete(),
                    "seed {seed} did not make a valid answer"
                );
                assert_eq!(
                    count_solutions(&puzzle.clues, 2),
                    1,
                    "seed {seed} at {difficulty:?} failed uniqueness"
                );
                assert_eq!(solve(&puzzle.clues), Some(puzzle.solution));
                // Every clue is consistent with the answer. A puzzle that
                // stalled just short of its target may hold a few more clues;
                // it can never hold fewer than 17, the most any sudoku has been
                // reduced to.
                assert!(puzzle.clues.filled() <= difficulty.clues() + 5);
                assert!(puzzle.clues.filled() >= 17);
                for index in 0..CELLS {
                    if !puzzle.clues.is_empty(index) {
                        assert_eq!(puzzle.clues.get(index), puzzle.solution.get(index));
                    }
                }
            }
        }
    }

    #[test]
    fn harder_difficulties_have_fewer_clues() {
        let easy = generate(Difficulty::Easy, 7).clues.filled();
        let medium = generate(Difficulty::Medium, 7).clues.filled();
        let hard = generate(Difficulty::Hard, 7).clues.filled();
        assert!(easy > medium, "easy {easy} medium {medium}");
        assert!(medium > hard, "medium {medium} hard {hard}");
    }

    #[test]
    fn the_same_seed_is_the_same_puzzle_and_a_different_seed_is_not() {
        let first = generate(Difficulty::Medium, 42);
        let again = generate(Difficulty::Medium, 42);
        assert_eq!(first, again);
        let other = generate(Difficulty::Medium, 43);
        assert_ne!(first.clues, other.clues);
    }

    #[test]
    fn the_generator_is_bounded_work() {
        // Uniqueness is proven for every clue that is removed; this is the one
        // test that would catch a generator that stopped terminating. Ten
        // puzzles is a second of work at worst.
        let started = std::time::Instant::now();
        for seed in 100..104u64 {
            let puzzle = generate(Difficulty::Hard, seed);
            assert_eq!(count_solutions(&puzzle.clues, 2), 1);
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "generating four hard puzzles took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn difficulty_names_round_trip_independently_of_their_wording() {
        for difficulty in Difficulty::ALL {
            assert_eq!(Difficulty::from_slug(difficulty.slug()), Some(difficulty));
            assert!(!difficulty.label().is_empty());
            assert_eq!(difficulty.next().next().next(), difficulty);
        }
        assert_eq!(Difficulty::from_slug("impossible"), None);
        assert_eq!(Difficulty::from_slug("Easy"), None);
    }

    #[test]
    fn a_solved_grid_is_only_solved_by_its_own_answer() {
        let answer = solve(&grid(HARD)).expect("one answer");
        assert!(answer.is_solved_as(&answer));
        assert!(!Grid::empty().is_solved_as(&Grid::empty()));
        let mut elsewhere = answer;
        elsewhere.set(0, 1);
        assert!(!elsewhere.is_solved_as(&answer));
    }

    #[test]
    fn the_rng_never_stalls_and_stays_in_range() {
        let mut rng = Rng::new(0);
        let mut seen = Vec::new();
        for _ in 0..1000 {
            seen.push(rng.next_u64());
        }
        assert!(seen.iter().any(|&value| value != seen[0]));
        for bound in [0usize, 1, 2, 9, 81] {
            for _ in 0..100 {
                assert!(rng.below(bound) < bound.max(1));
            }
        }
    }
}
