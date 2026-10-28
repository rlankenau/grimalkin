//! Screen storage: visible lines plus (optional) scrollback ring.

use crate::cell::{Cell, Color, Row};
use std::collections::VecDeque;

/// A screen grid. The primary screen carries a scrollback ring; the alternate
/// screen is created with a scrollback limit of 0.
#[derive(Debug, Clone)]
pub struct Grid {
    cols: usize,
    lines: Vec<Row>,
    scrollback: VecDeque<Row>,
    scrollback_limit: usize,
    /// Number of lines that have ever scrolled off the top of the screen
    /// (whether or not they were kept in scrollback).
    scrolled_total: u64,
}

impl Grid {
    pub fn new(cols: usize, rows: usize, scrollback_limit: usize) -> Grid {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Grid {
            cols,
            lines: (0..rows).map(|_| Row::new(cols, Color::Default)).collect(),
            scrollback: VecDeque::new(),
            scrollback_limit,
            scrolled_total: 0,
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn rows(&self) -> usize {
        self.lines.len()
    }
    pub fn row(&self, r: usize) -> &Row {
        &self.lines[r]
    }
    pub fn row_mut(&mut self, r: usize) -> &mut Row {
        &mut self.lines[r]
    }
    pub fn cell(&self, r: usize, c: usize) -> &Cell {
        &self.lines[r].cells[c]
    }
    pub fn cell_mut(&mut self, r: usize, c: usize) -> &mut Cell {
        &mut self.lines[r].cells[c]
    }
    pub fn lines(&self) -> &[Row] {
        &self.lines
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }
    pub fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }
    /// Scrollback row by index, 0 = oldest.
    pub fn scrollback_row(&self, i: usize) -> Option<&Row> {
        self.scrollback.get(i)
    }
    pub fn scrolled_total(&self) -> u64 {
        self.scrolled_total
    }
    pub(crate) fn reset_scrolled_total(&mut self) {
        self.scrolled_total = 0;
    }
    pub(crate) fn clear_scrollback(&mut self) {
        self.scrollback.clear();
    }

    /// Visit every row, visible and scrollback, mutably.
    pub(crate) fn for_each_row_mut(&mut self, mut f: impl FnMut(&mut Row)) {
        self.lines.iter_mut().for_each(&mut f);
        self.scrollback.iter_mut().for_each(&mut f);
    }

    pub fn clear(&mut self, bg: Color) {
        for l in &mut self.lines {
            l.clear(bg);
        }
    }

    /// Scroll lines `top..=bottom` up by `n`. Lines leaving the top of a
    /// region that starts at row 0 enter scrollback. Returns the number of
    /// lines that went into scrollback (kept).
    pub fn scroll_up(&mut self, top: usize, bottom: usize, n: usize, bg: Color) -> usize {
        let height = bottom - top + 1;
        let n = n.min(height);
        if n == 0 {
            return 0;
        }
        let cols = self.cols;
        let mut removed = Vec::with_capacity(n);
        for i in 0..n {
            removed.push(std::mem::replace(
                &mut self.lines[top + i],
                Row::new(cols, bg),
            ));
        }
        self.lines[top..=bottom].rotate_left(n);
        // The rotated-in blank rows must use the erase background; they were
        // created with `bg` above.
        let mut kept = 0;
        if top == 0 {
            self.scrolled_total += n as u64;
            if self.scrollback_limit > 0 {
                for r in removed {
                    self.scrollback.push_back(r);
                    kept += 1;
                    if self.scrollback.len() > self.scrollback_limit {
                        self.scrollback.pop_front();
                        kept -= 1;
                    }
                }
            }
        }
        kept
    }

    /// Scroll lines `top..=bottom` down by `n`, inserting blank lines at top.
    pub fn scroll_down(&mut self, top: usize, bottom: usize, n: usize, bg: Color) {
        let height = bottom - top + 1;
        let n = n.min(height);
        if n == 0 {
            return;
        }
        let cols = self.cols;
        for i in 0..n {
            self.lines[bottom - i] = Row::new(cols, bg);
        }
        self.lines[top..=bottom].rotate_right(n);
    }

    /// Insert `n` blank lines at `at`, pushing lines down within `..=bottom`.
    pub fn insert_lines(&mut self, at: usize, bottom: usize, n: usize, bg: Color) {
        self.scroll_down(at, bottom, n, bg);
    }

    /// Delete `n` lines at `at`, pulling lines up within `..=bottom`.
    /// Never touches scrollback.
    pub fn delete_lines(&mut self, at: usize, bottom: usize, n: usize, bg: Color) {
        let height = bottom - at + 1;
        let n = n.min(height);
        if n == 0 {
            return;
        }
        let cols = self.cols;
        for i in 0..n {
            self.lines[at + i] = Row::new(cols, bg);
        }
        self.lines[at..=bottom].rotate_left(n);
    }

    /// Resize without reflow. `cursor_row` is the cursor's row; the returned
    /// value is the number of rows the cursor must shift (negative = up) by,
    /// because lines moved to/from scrollback.
    pub fn resize(&mut self, cols: usize, rows: usize, cursor_row: usize) -> isize {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols != self.cols {
            for l in &mut self.lines {
                l.resize(cols);
            }
            for l in &mut self.scrollback {
                l.resize(cols);
            }
            self.cols = cols;
        }
        let cur = self.lines.len();
        let mut shift: isize = 0;
        if rows < cur {
            // Keep the cursor visible by pushing top lines into scrollback.
            let need = (cursor_row + 1).saturating_sub(rows);
            for _ in 0..need {
                let row = self.lines.remove(0);
                self.scrolled_total += 1;
                if self.scrollback_limit > 0 {
                    self.scrollback.push_back(row);
                    if self.scrollback.len() > self.scrollback_limit {
                        self.scrollback.pop_front();
                    }
                }
                shift -= 1;
            }
            self.lines.truncate(rows);
        } else if rows > cur {
            let mut extra = rows - cur;
            // Pull lines back out of scrollback.
            while extra > 0 {
                if let Some(row) = self.scrollback.pop_back() {
                    self.lines.insert(0, row);
                    self.scrolled_total = self.scrolled_total.saturating_sub(1);
                    shift += 1;
                    extra -= 1;
                } else {
                    break;
                }
            }
            for _ in 0..extra {
                self.lines.push(Row::new(cols, Color::Default));
            }
        }
        shift
    }
}
