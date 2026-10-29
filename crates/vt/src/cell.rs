//! Cell, colour and attribute model.

/// A terminal colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// The terminal's default foreground/background.
    #[default]
    Default,
    /// Index into the 256-colour palette.
    Indexed(u8),
    /// 24-bit colour.
    Rgb(u8, u8, u8),
}

/// Underline style (SGR 4, 4:x, 21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// Boolean text attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags(pub u16);

impl Flags {
    pub const BOLD: Flags = Flags(1 << 0);
    pub const DIM: Flags = Flags(1 << 1);
    pub const ITALIC: Flags = Flags(1 << 2);
    pub const STRIKETHROUGH: Flags = Flags(1 << 3);
    pub const INVERSE: Flags = Flags(1 << 4);
    pub const HIDDEN: Flags = Flags(1 << 5);
    pub const BLINK: Flags = Flags(1 << 6);
    /// Internal marker, not an SGR attribute: the blank left at the end of a
    /// soft-wrapped row because a wide character did not fit there. Lets
    /// copy / URL detection tell it from a typed space.
    pub const WRAP_PAD: Flags = Flags(1 << 7);

    pub const fn empty() -> Flags {
        Flags(0)
    }
    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn insert(&mut self, other: Flags) {
        self.0 |= other.0;
    }
    pub fn remove(&mut self, other: Flags) {
        self.0 &= !other.0;
    }
    pub fn set(&mut self, other: Flags, on: bool) {
        if on {
            self.insert(other)
        } else {
            self.remove(other)
        }
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Flags;
    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

/// Whether a cell is a normal cell, the left half of a wide character, or
/// the (empty) right half of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CellWidth {
    #[default]
    Single,
    /// Leftmost cell of a double-width character.
    Wide,
    /// Right half of a double-width character; carries no content.
    WideSpacer,
}

/// Drawing attributes applied to newly printed cells ("pen").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
    pub underline: Underline,
    pub underline_color: Color,
    pub hyperlink_id: Option<u32>,
}

/// Combining marks of a cell. Nearly all cells have none, so this is a single
/// (null) pointer rather than an inline `Vec`: 16 bytes less per cell, which
/// is what scrollback is made of.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(clippy::box_collection)] // deliberate: a thin pointer, not a 24-byte Vec
pub struct Combining(Option<Box<Vec<char>>>);

impl Combining {
    pub const fn new() -> Combining {
        Combining(None)
    }
    pub fn push(&mut self, c: char) {
        self.0.get_or_insert_with(Default::default).push(c);
    }
}

impl std::ops::Deref for Combining {
    type Target = [char];
    fn deref(&self) -> &[char] {
        match &self.0 {
            Some(v) => v,
            None => &[],
        }
    }
}

impl<'a> IntoIterator for &'a Combining {
    type Item = &'a char;
    type IntoIter = std::slice::Iter<'a, char>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl PartialEq<Vec<char>> for Combining {
    fn eq(&self, other: &Vec<char>) -> bool {
        **self == other[..]
    }
}

/// One grid cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// Base character (`' '` for empty cells).
    pub ch: char,
    /// Zero-width combining characters / grapheme extension that follow `ch`.
    pub combining: Combining,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
    pub underline: Underline,
    pub underline_color: Color,
    pub hyperlink_id: Option<u32>,
    pub width: CellWidth,
}

impl Default for Cell {
    fn default() -> Self {
        Cell::blank()
    }
}

impl Cell {
    pub const fn blank() -> Cell {
        Cell {
            ch: ' ',
            combining: Combining::new(),
            fg: Color::Default,
            bg: Color::Default,
            flags: Flags::empty(),
            underline: Underline::None,
            underline_color: Color::Default,
            hyperlink_id: None,
            width: CellWidth::Single,
        }
    }

    /// An erased cell: blank with only the given background kept.
    pub fn erased(bg: Color) -> Cell {
        Cell {
            bg,
            ..Cell::blank()
        }
    }

    pub fn with_style(ch: char, style: &Style, width: CellWidth) -> Cell {
        Cell {
            ch,
            combining: Combining::new(),
            fg: style.fg,
            bg: style.bg,
            flags: style.flags,
            underline: style.underline,
            underline_color: style.underline_color,
            hyperlink_id: style.hyperlink_id,
            width,
        }
    }

    pub fn style(&self) -> Style {
        Style {
            fg: self.fg,
            bg: self.bg,
            flags: self.flags,
            underline: self.underline,
            underline_color: self.underline_color,
            hyperlink_id: self.hyperlink_id,
        }
    }

    pub fn is_wide(&self) -> bool {
        self.width == CellWidth::Wide
    }
    pub fn is_spacer(&self) -> bool {
        self.width == CellWidth::WideSpacer
    }
    /// True when the cell shows nothing (space, no combining marks).
    pub fn is_empty(&self) -> bool {
        self.ch == ' ' && self.combining.is_empty() && self.width == CellWidth::Single
    }
    /// Append the cell's text (base char + combining) to `out`.
    pub fn push_text(&self, out: &mut String) {
        out.push(self.ch);
        out.extend(self.combining.iter());
    }
    pub fn bold(&self) -> bool {
        self.flags.contains(Flags::BOLD)
    }
    pub fn inverse(&self) -> bool {
        self.flags.contains(Flags::INVERSE)
    }
}

/// Kind of OSC 133 shell-integration mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkKind {
    /// `A`: prompt start.
    PromptStart,
    /// `B`: prompt end / command input start.
    PromptEnd,
    /// `C`: command output start.
    OutputStart,
    /// `D`: command finished.
    CommandEnd,
}

/// A shell-integration mark attached to a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMark {
    pub kind: MarkKind,
    /// Exit status for [`MarkKind::CommandEnd`] when supplied.
    pub exit_code: Option<i32>,
}

/// A row of cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// True when this row soft-wrapped into the next one.
    pub wrapped: bool,
    /// OSC 133 marks placed on this row.
    pub marks: Vec<LineMark>,
}

impl Row {
    pub fn new(cols: usize, bg: Color) -> Row {
        Row {
            cells: vec![Cell::erased(bg); cols],
            wrapped: false,
            marks: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn clear(&mut self, bg: Color) {
        let blank = Cell::erased(bg);
        for c in &mut self.cells {
            *c = blank.clone();
        }
        self.wrapped = false;
        self.marks.clear();
    }

    /// Plain text of the row (spacers skipped, trailing blanks trimmed).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for c in &self.cells {
            if c.is_spacer() {
                continue;
            }
            c.push_text(&mut s);
        }
        let trimmed = s.trim_end_matches(' ').len();
        s.truncate(trimmed);
        s
    }

    /// Resize the row, truncating or extending with blank cells.
    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::blank());
        // Rows are consistent on entry, so the only wide cell that can have
        // lost its spacer is one now sitting in the last column.
        if let Some(last) = self.cells.last_mut() {
            if last.width == CellWidth::Wide {
                *last = Cell::erased(last.bg);
            }
        }
    }

    /// Repair orphaned wide cells / spacers anywhere in the row.
    pub fn fix_wide(&mut self) {
        let n = self.cells.len();
        for i in 0..n {
            match self.cells[i].width {
                CellWidth::Wide => {
                    if i + 1 >= n || self.cells[i + 1].width != CellWidth::WideSpacer {
                        let bg = self.cells[i].bg;
                        self.cells[i] = Cell::erased(bg);
                    }
                }
                CellWidth::WideSpacer => {
                    if i == 0 || self.cells[i - 1].width != CellWidth::Wide {
                        let bg = self.cells[i].bg;
                        self.cells[i] = Cell::erased(bg);
                    }
                }
                CellWidth::Single => {}
            }
        }
    }
}

/// Default xterm-style 256-colour palette value.
pub fn default_palette_rgb(idx: u8) -> (u8, u8, u8) {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    match idx {
        0..=15 => ANSI[idx as usize],
        16..=231 => {
            let i = idx - 16;
            let lvl = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            (lvl(i / 36), lvl((i / 6) % 6), lvl(i % 6))
        }
        232..=255 => {
            let v = 8 + 10 * (idx - 232);
            (v, v, v)
        }
    }
}
