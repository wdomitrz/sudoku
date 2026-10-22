//! Sudoku: a puzzle that works offline, and nothing else.
//!
//! The split this crate is built on, and the reason it is two thirds Rust:
//!
//! * [`board`] — the puzzle itself: the grid, the rules, a solver and a
//!   generator that proves every puzzle it makes has exactly one answer.
//! * [`game`] — one sitting of it: the selection, the digits written, the
//!   pencil marks, the clock, the moves that can be taken back.
//! * `ui` — the browser layer, compiled only for `wasm32-unknown-unknown`
//!   (so it is absent from a host build, and a doc link to it would not
//!   resolve there).
//!   Every `document`, `localStorage` and service-worker call lives here, and
//!   nothing else does.
//!
//! The first two have no `web-sys`, no `js-sys` and no `wasm-bindgen`, so
//! `cargo test` compiles them natively and runs the whole rule set without a
//! browser. That is the point: the moment the puzzle reaches for `document`,
//! its tests stop running and the app becomes a blob.
//!
//! `cargo build --release` writes the whole site to `dist/` through `build.rs`.
//! There is no server, no binary and no run step: the build is the product.

#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

pub mod board;
pub mod game;

#[cfg(target_arch = "wasm32")]
pub mod ui;

pub use board::{Difficulty, Grid, Puzzle};
pub use game::{Game, Readout, Report};
