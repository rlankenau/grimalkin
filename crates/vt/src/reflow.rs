//! Reflow of the primary screen (scrollback plus visible rows) when the
//! column count changes.
//!
//! Consecutive rows linked by `Row::wrapped` form one *logical line*. Each
//! logical line is joined, its trailing default blanks are trimmed (so blank
//! space at the end of a line never turns into a spurious wrap), and it is
//! split again at the new width. A wide character is never split: when it
//! would straddle the right edge it moves to the next row and leaves a
//! `Flags::WRAP_PAD` blank behind, exactly like the writer in `term.rs`.
//!
//! Cost is O(cells): every cell is moved at most once, and an ordinary
//! unwrapped line that still fits is only truncated / padded in place.
//!
//! Absolute row numbers (`Grid::scrolled_total`) are not preserved by a
//! reflow, because the number of rows each line occupies changes. The
//! cursor's absolute row never decreases, but anything anchored to older
//! absolute rows (image placements, selections) is invalid afterwards; the
//! terminal announces that with `TermEvent::Reflowed`.

use crate::cell::{Cell, Color, Flags, LineMark, Row};
use crate::grid::Grid;

/// A position (visible row, column) that must follow its character through
/// the reflow. `pending` is the deferred-wrap state: the position is just
/// after the last character written, at the end of the row.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Track {
    pub row: usize,
    pub col: usize,
    pub pending: bool,
}

struct Pend {
    track: usize,
    /// Offset into the joined logical line.
    off: usize,
    /// True: the position is *after* cell `off - 1` (deferred wrap).
    after: bool,
}

/// Position in the new full row list: (row index, column, pending wrap).
type Pos = (usize, usize, bool);

fn dedupe_marks(marks: Vec<LineMark>) -> Vec<LineMark> {
    let mut out: Vec<LineMark> = Vec::with_capacity(marks.len());
    for m in marks {
        match out.iter_mut().find(|o| o.kind == m.kind) {
            Some(o) => *o = m,
            None => out.push(m),
        }
    }
    out
}

fn push_row(out: &mut Vec<Row>, mut cells: Vec<Cell>, new_cols: usize, wrapped: bool) {
    cells.resize(new_cols, Cell::blank());
    out.push(Row { cells, wrapped, marks: Vec::new() });
}

/// Split one joined logical line into rows of `new_cols` cells.
fn layout(
    line: &mut Vec<Cell>,
    marks: Vec<LineMark>,
    new_cols: usize,
    pend: &[Pend],
    out: &mut Vec<Row>,
    pos: &mut [Pos],
) {
    let first = out.len();
    for p in pend {
        if p.after && p.off == 0 {
            pos[p.track] = (first, 0, false);
        }
    }
    let mut cur: Vec<Cell> = Vec::with_capacity(new_cols);
    let mut col = 0usize;
    let mut src = 0usize;
    let mut it = line.drain(..).peekable();
    while let Some(cell) = it.next() {
        let wide = cell.is_wide();
        let mut spacer = None;
        if wide && it.peek().is_some_and(|s| s.is_spacer()) {
            spacer = it.next();
        }
        // Orphans (a wide cell with no spacer, a stray spacer) and wide
        // characters on a 1-column screen become a plain blank.
        let (cell, spacer, w, consumed) = if spacer.is_some() {
            if new_cols < 2 {
                (Cell::erased(cell.bg), None, 1, 2)
            } else {
                (cell, spacer, 2, 2)
            }
        } else if wide || cell.is_spacer() {
            (Cell::erased(cell.bg), None, 1, 1)
        } else {
            (cell, None, 1, 1)
        };
        if col + w > new_cols {
            if w == 2 && col < new_cols {
                let mut pad = Cell::erased(cell.bg);
                pad.flags.insert(Flags::WRAP_PAD);
                cur.push(pad);
            }
            push_row(out, std::mem::replace(&mut cur, Vec::with_capacity(new_cols)), new_cols, true);
            col = 0;
        }
        let row_ix = out.len();
        for p in pend {
            if !p.after && p.off == src {
                pos[p.track] = (row_ix, col, false);
            }
        }
        cur.push(cell);
        if let Some(s) = spacer {
            cur.push(s);
        }
        col += w;
        src += consumed;
        for p in pend {
            if p.after && p.off > src - consumed && p.off <= src {
                pos[p.track] = if col >= new_cols { (row_ix, new_cols - 1, true) } else { (row_ix, col, false) };
            }
        }
    }
    drop(it);
    push_row(out, cur, new_cols, false);
    out[first].marks = dedupe_marks(marks);
}

impl Grid {
    /// Rewrap the whole grid (scrollback and visible rows) to `new_cols`
    /// columns and `new_rows` visible rows. `track[0]` is the cursor; every
    /// entry is updated in place to its character's new visible position.
    ///
    /// The visible window is chosen to keep the cursor on its old screen row
    /// when possible (a taller screen pulls history back in), show any
    /// content below the cursor, and always keep the cursor visible. The
    /// scrollback limit is honoured by dropping the oldest rows.
    #[allow(unused_assignments)]
    pub(crate) fn reflow(&mut self, new_cols: usize, new_rows: usize, track: &mut [Track]) {
        let new_cols = new_cols.max(1);
        let new_rows = new_rows.max(1);
        let old_rows = self.lines.len();
        let sb_len = self.scrollback.len();
        let r_c = track[0].row.min(old_rows - 1);
        let old_cursor_abs = self.scrolled_total + r_c as u64;
        let tidx: Vec<usize> = track.iter().map(|t| sb_len + t.row.min(old_rows - 1)).collect();

        let old_scroll = std::mem::take(&mut self.scrollback);
        let old_lines = std::mem::take(&mut self.lines);
        let mut out: Vec<Row> = Vec::with_capacity(sb_len + old_rows + 16);
        let mut pos: Vec<Pos> = vec![(0, 0, false); track.len()];
        let mut last_content: Option<usize> = None;

        let blank = Cell::blank();
        let mut line: Vec<Cell> = Vec::new();
        let mut marks: Vec<LineMark> = Vec::new();
        let mut pend: Vec<Pend> = Vec::new();
        let mut line_rows = 0usize;
        // New index of the first row of the logical line that owned the old
        // top visible row: the text that was at the top stays there.
        let mut line_start = 0usize;
        let mut old_top_new = 0usize;
        // The old screen reached its last row (so a taller / wider screen
        // should pull history back in rather than leave a gap).
        let screen_full = tidx[0] == sb_len + old_rows - 1
            || old_lines.last().is_some_and(|r| r.cells.iter().any(|c| *c != blank) || !r.marks.is_empty());

        macro_rules! flush {
            () => {{
                while line.last().is_some_and(|c| *c == blank) {
                    line.pop();
                }
                let nonempty = !line.is_empty() || !marks.is_empty();
                // A cursor just past the end of the text is "after the last
                // character": at the right edge that is a deferred wrap, not
                // a new (blank) row.
                for p in pend.iter_mut() {
                    if !p.after && p.off == line.len() {
                        p.after = true;
                    }
                }
                let need = pend.iter().map(|p| if p.after { p.off } else { p.off + 1 }).max().unwrap_or(0);
                if line.len() < need {
                    line.resize(need, Cell::blank());
                }
                layout(&mut line, std::mem::take(&mut marks), new_cols, &pend, &mut out, &mut pos);
                if nonempty {
                    last_content = Some(out.len() - 1);
                }
                pend.clear();
                line_rows = 0;
            }};
        }

        for (i, mut row) in old_scroll.into_iter().chain(old_lines).enumerate() {
            let mut cells = std::mem::take(&mut row.cells);
            if row.wrapped && cells.last().is_some_and(|c| c.flags.contains(Flags::WRAP_PAD)) {
                cells.pop();
            }
            let tracked = tidx.contains(&i);
            if line_rows == 0 {
                line_start = out.len();
            }
            if i == sb_len {
                old_top_new = line_start;
            }
            if line_rows == 0 && !row.wrapped && !tracked {
                // Fast path: a standalone row that still fits is edited in place.
                while cells.last().is_some_and(|c| *c == blank) {
                    cells.pop();
                }
                if cells.len() <= new_cols {
                    let nonempty = !cells.is_empty() || !row.marks.is_empty();
                    cells.resize(new_cols, Cell::blank());
                    out.push(Row { cells, wrapped: false, marks: row.marks });
                    if nonempty {
                        last_content = Some(out.len() - 1);
                    }
                    continue;
                }
            }
            for (k, &ti) in tidx.iter().enumerate() {
                if ti == i {
                    let t = track[k];
                    let (c, after) =
                        if t.pending { ((t.col + 1).min(cells.len()), true) } else { (t.col.min(cells.len()), false) };
                    pend.push(Pend { track: k, off: line.len() + c, after });
                }
            }
            line.append(&mut cells);
            marks.append(&mut row.marks);
            line_rows += 1;
            if !row.wrapped {
                flush!();
            }
        }
        if line_rows > 0 {
            flush!();
        }

        // Pick the visible window.
        let c_idx = pos[0].0;
        let end = last_content.map_or(0, |l| l + 1).max(c_idx + 1);
        out.truncate(end);
        let total = out.len();
        let mut top = old_top_new.min(c_idx);
        if c_idx >= top + new_rows {
            top = c_idx + 1 - new_rows;
        }
        // Content below the cursor is worth showing, but never at the cost
        // of the cursor; a full old screen pulls history in to fill a taller
        // one instead of leaving a gap.
        let fill = total.saturating_sub(new_rows).min(c_idx);
        if fill > top || screen_full {
            top = fill;
        }

        let mut visible = out.split_off(top);
        visible.truncate(new_rows);
        while visible.len() < new_rows {
            visible.push(Row::new(new_cols, Color::Default));
        }
        let drop_n = out.len().saturating_sub(self.scrollback_limit);
        out.drain(..drop_n);
        let sb_final = out.len() as u64;

        for (t, p) in track.iter_mut().zip(&pos) {
            t.row = p.0.saturating_sub(top).min(new_rows - 1);
            t.col = p.1;
            t.pending = p.2;
        }

        // Keep the cursor's absolute row monotonic.
        self.scrolled_total = self
            .scrolled_total
            .max(old_cursor_abs.saturating_sub(track[0].row as u64))
            .max(sb_final);
        self.cols = new_cols;
        self.lines = visible;
        self.scrollback = out.into();
    }
}
