// SPDX-License-Identifier: AGPL-3.0-or-later

//! Terminal grid model — a cell-based buffer with cursor, scroll regions,
//! dirty tracking, and pen state.  This is the core state machine that a
//! VTE handler writes into and a renderer reads from.

use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

/// Terminal color representation.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum Color {
    /// Use the terminal's default foreground / background.
    #[default]
    Default,
    /// One of the 256-color palette entries (0–255).
    Indexed(u8),
    /// 24-bit true color.
    Rgb(u8, u8, u8),
}

impl std::fmt::Debug for Color {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => write!(f, "Default"),
            Self::Indexed(i) => write!(f, "Indexed({i})"),
            Self::Rgb(r, g, b) => write!(f, "Rgb({r},{g},{b})"),
        }
    }
}


// ---------------------------------------------------------------------------
// CellAttrs — bitflag newtype
// ---------------------------------------------------------------------------

/// Cell-level text attributes stored as a bitfield.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct CellAttrs(pub u8);

impl CellAttrs {
    pub const BOLD: u8 = 0x01;
    pub const DIM: u8 = 0x02;
    pub const ITALIC: u8 = 0x04;
    pub const UNDERLINE: u8 = 0x08;
    pub const INVERSE: u8 = 0x10;
    pub const STRIKETHROUGH: u8 = 0x20;

    /// Returns `true` if the given flag bit(s) are set.
    pub fn contains(self, flag: u8) -> bool {
        self.0 & flag == flag
    }

    /// Set the given flag bit(s).
    pub fn insert(&mut self, flag: u8) {
        self.0 |= flag;
    }

    /// Clear the given flag bit(s).
    pub fn remove(&mut self, flag: u8) {
        self.0 &= !flag;
    }

    /// Returns `true` when no attribute bits are set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

// ---------------------------------------------------------------------------
// Cell
// ---------------------------------------------------------------------------

/// A single character cell in the terminal grid.
#[derive(Clone, Debug)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: CellAttrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: CellAttrs::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// CursorState
// ---------------------------------------------------------------------------

/// Cursor position and saved state for DECSC / DECRC.
#[derive(Clone, Debug)]
pub struct CursorState {
    pub row: usize,
    pub col: usize,
    pub saved_row: usize,
    pub saved_col: usize,
    pub saved_fg: Color,
    pub saved_bg: Color,
    pub saved_attrs: CellAttrs,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            saved_row: 0,
            saved_col: 0,
            saved_fg: Color::Default,
            saved_bg: Color::Default,
            saved_attrs: CellAttrs::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// TerminalModes
// ---------------------------------------------------------------------------

/// DEC private-mode and ANSI-mode flags.
#[derive(Debug, Clone)]
pub struct TerminalModes {
    /// DECCKM — application cursor keys (?1).
    pub application_cursor_keys: bool,
    /// DECAWM — auto-wrap mode (?7).  Default **true**.
    pub auto_wrap: bool,
    /// DECOM — origin mode (?6).
    pub origin_mode: bool,
    /// IRM — insert / replace mode (4).
    pub insert_mode: bool,
    /// Alternate screen buffer (?1049 / ?47).
    pub alternate_screen: bool,
    /// Bracketed paste (?2004).
    pub bracketed_paste: bool,
    /// DECTCEM — cursor visible (?25).  Default **true**.
    pub cursor_visible: bool,
}

impl Default for TerminalModes {
    fn default() -> Self {
        Self {
            application_cursor_keys: false,
            auto_wrap: true,
            origin_mode: false,
            insert_mode: false,
            alternate_screen: false,
            bracketed_paste: false,
            cursor_visible: true,
        }
    }
}

// ---------------------------------------------------------------------------
// StyledSpan — renderer output
// ---------------------------------------------------------------------------

/// A run of consecutive characters that share the same style.
/// Produced by [`Grid::row_to_styled_spans`].
#[derive(Debug, Clone)]
pub struct StyledSpan {
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub attrs: CellAttrs,
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

/// The terminal's visible cell grid plus scrollback, cursor, modes, and pen.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// Visible grid — `cells[row][col]`.
    pub cells: Vec<Vec<Cell>>,
    /// Lines that have scrolled off the top of the visible area.
    pub scrollback: Vec<Vec<Cell>>,
    /// Maximum number of scrollback lines retained.
    pub max_scrollback: usize,
    pub cursor: CursorState,
    pub modes: TerminalModes,
    /// Row indices that have been modified since the last render.
    pub dirty_rows: HashSet<usize>,
    /// Top of the scroll region (inclusive, 0-based).
    pub scroll_top: usize,
    /// Bottom of the scroll region (inclusive, 0-based).
    pub scroll_bottom: usize,
    /// Pen foreground for newly written characters.
    pub current_fg: Color,
    /// Pen background for newly written characters.
    pub current_bg: Color,
    /// Pen attributes for newly written characters.
    pub current_attrs: CellAttrs,
    /// Column-indexed tab-stop positions (`true` = tab stop present).
    pub tab_stops: Vec<bool>,
    /// Terminal title set via OSC escape.
    pub title: String,
    /// Bytes queued to send back to the PTY (e.g. device-status responses).
    pub response_bytes: Vec<u8>,
}

impl Grid {
    // -- construction -------------------------------------------------------

    /// Create a new grid of `cols` x `rows` blank cells with default tab stops
    /// every 8 columns.
    pub fn new(cols: usize, rows: usize) -> Self {
        let cells = blank_grid(cols, rows);
        let tab_stops = default_tab_stops(cols);

        let mut dirty_rows = HashSet::new();
        for r in 0..rows {
            dirty_rows.insert(r);
        }

        Self {
            cols,
            rows,
            cells,
            scrollback: Vec::new(),
            max_scrollback: 10_000,
            cursor: CursorState::default(),
            modes: TerminalModes::default(),
            dirty_rows,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            current_fg: Color::Default,
            current_bg: Color::Default,
            current_attrs: CellAttrs::default(),
            tab_stops,
            title: String::new(),
            response_bytes: Vec::new(),
        }
    }

    // -- resize -------------------------------------------------------------

    /// Resize the grid to new dimensions, preserving as much content as
    /// possible.  Marks every row dirty.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == 0 || rows == 0 {
            return;
        }

        // Adjust existing rows to the new column width.
        for row in &mut self.cells {
            row.resize_with(cols, Cell::default);
            row.truncate(cols);
        }

        // Add or remove rows.
        if rows > self.rows {
            // Try to pull lines back from scrollback first.
            while self.cells.len() < rows {
                if let Some(line) = self.scrollback.pop() {
                    let mut line = line;
                    line.resize_with(cols, Cell::default);
                    line.truncate(cols);
                    self.cells.insert(0, line);
                    // Cursor shifts down because we inserted above.
                    self.cursor.row = self.cursor.row.saturating_add(1);
                } else {
                    self.cells.push(blank_row(cols));
                }
            }
        } else if rows < self.rows {
            // Push excess top rows into scrollback.
            while self.cells.len() > rows {
                let line = self.cells.remove(0);
                self.scrollback.push(line);
                self.cursor.row = self.cursor.row.saturating_sub(1);
            }
            trim_scrollback(&mut self.scrollback, self.max_scrollback);
        }

        self.cols = cols;
        self.rows = rows;
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
        self.tab_stops = default_tab_stops(cols);

        // Clamp cursor.
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(cols.saturating_sub(1));

        self.mark_all_dirty();
    }

    // -- character output ---------------------------------------------------

    /// Write a character at the cursor position using the current pen style.
    ///
    /// In insert mode the character is inserted (existing cells shift right).
    /// If auto-wrap is on and the cursor is past the last column, it wraps to
    /// the next line first.
    pub fn put_char(&mut self, c: char) {
        // Auto-wrap: if the cursor is already at (or past) the last column
        // after a previous put_char, wrap now.
        if self.cursor.col >= self.cols {
            if self.modes.auto_wrap {
                self.cursor.col = 0;
                self.newline();
            } else {
                self.cursor.col = self.cols.saturating_sub(1);
            }
        }

        let row = self.cursor.row;
        let col = self.cursor.col;

        if self.modes.insert_mode {
            // Shift cells right from cursor.
            let r = &mut self.cells[row];
            r.pop(); // keep row length fixed
            r.insert(col, Cell::default());
        }

        self.cells[row][col] = Cell {
            c,
            fg: self.current_fg,
            bg: self.current_bg,
            attrs: self.current_attrs,
        };
        self.dirty_rows.insert(row);

        self.cursor.col += 1;
        // We allow cursor.col == self.cols (one past last column) — the
        // actual wrap happens on the *next* put_char or explicit newline.
    }

    // -- line movement ------------------------------------------------------

    /// Move cursor to the next line.  If the cursor is at the bottom of the
    /// scroll region, scroll the region up instead.
    pub fn newline(&mut self) {
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.row < self.rows.saturating_sub(1) {
            self.cursor.row += 1;
        }
    }

    /// Move cursor to column 0.
    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
    }

    /// Move cursor one position to the left (minimum column 0).
    pub fn backspace(&mut self) {
        self.cursor.col = self.cursor.col.saturating_sub(1);
    }

    /// Advance the cursor to the next tab stop, or to the last column.
    pub fn tab(&mut self) {
        let start = self.cursor.col + 1;
        for i in start..self.cols {
            if self.tab_stops[i] {
                self.cursor.col = i;
                return;
            }
        }
        self.cursor.col = self.cols.saturating_sub(1);
    }

    // -- cursor movement (CSI sequences) ------------------------------------

    /// Move cursor up by `n` rows, clamped to the scroll region top (if
    /// origin mode) or row 0.
    pub fn cursor_up(&mut self, n: usize) {
        let top = if self.modes.origin_mode {
            self.scroll_top
        } else {
            0
        };
        self.cursor.row = self.cursor.row.saturating_sub(n).max(top);
    }

    /// Move cursor down by `n` rows, clamped to the scroll region bottom (if
    /// origin mode) or the last row.
    pub fn cursor_down(&mut self, n: usize) {
        let bottom = if self.modes.origin_mode {
            self.scroll_bottom
        } else {
            self.rows.saturating_sub(1)
        };
        self.cursor.row = (self.cursor.row + n).min(bottom);
    }

    /// Move cursor right by `n` columns (max `cols - 1`).
    pub fn cursor_forward(&mut self, n: usize) {
        self.cursor.col = (self.cursor.col + n).min(self.cols.saturating_sub(1));
    }

    /// Move cursor left by `n` columns (min 0).
    pub fn cursor_back(&mut self, n: usize) {
        self.cursor.col = self.cursor.col.saturating_sub(n);
    }

    /// Position cursor at the given 0-based (row, col), clamped to grid
    /// bounds.
    pub fn cursor_to(&mut self, row: usize, col: usize) {
        self.cursor.row = row.min(self.rows.saturating_sub(1));
        self.cursor.col = col.min(self.cols.saturating_sub(1));
    }

    // -- erase operations ---------------------------------------------------

    /// Erase parts of the display.
    ///
    /// | mode | effect |
    /// |------|--------|
    /// | 0    | cursor to end of screen |
    /// | 1    | start of screen to cursor |
    /// | 2    | entire screen |
    /// | 3    | entire screen + scrollback |
    pub fn erase_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // Cursor to end of current line.
                self.erase_cells_range(self.cursor.row, self.cursor.col, self.cols);
                // All lines below cursor.
                for r in (self.cursor.row + 1)..self.rows {
                    self.erase_cells_range(r, 0, self.cols);
                }
            }
            1 => {
                // All lines above cursor.
                for r in 0..self.cursor.row {
                    self.erase_cells_range(r, 0, self.cols);
                }
                // Start of current line to cursor (inclusive).
                self.erase_cells_range(self.cursor.row, 0, self.cursor.col + 1);
            }
            2 => {
                for r in 0..self.rows {
                    self.erase_cells_range(r, 0, self.cols);
                }
            }
            3 => {
                for r in 0..self.rows {
                    self.erase_cells_range(r, 0, self.cols);
                }
                self.scrollback.clear();
            }
            _ => {}
        }
    }

    /// Erase parts of the current line.
    ///
    /// | mode | effect |
    /// |------|--------|
    /// | 0    | cursor to end of line |
    /// | 1    | start of line to cursor |
    /// | 2    | entire line |
    pub fn erase_line(&mut self, mode: u16) {
        let row = self.cursor.row;
        match mode {
            0 => self.erase_cells_range(row, self.cursor.col, self.cols),
            1 => self.erase_cells_range(row, 0, self.cursor.col + 1),
            2 => self.erase_cells_range(row, 0, self.cols),
            _ => {}
        }
    }

    /// Clear `n` characters starting at the cursor position (ECH).
    pub fn erase_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let end = (self.cursor.col + n).min(self.cols);
        self.erase_cells_range(row, self.cursor.col, end);
    }

    // -- insert / delete characters -----------------------------------------

    /// Insert `n` blank characters at the cursor, shifting existing cells
    /// right.  Characters pushed past the end of the line are lost.
    pub fn insert_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        if col >= self.cols {
            return;
        }
        let r = &mut self.cells[row];
        for _ in 0..n {
            r.pop();
            r.insert(col, Cell::default());
        }
        self.dirty_rows.insert(row);
    }

    /// Delete `n` characters at the cursor, shifting remaining cells left.
    /// Blanks are appended at the end of the line.
    pub fn delete_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        if col >= self.cols {
            return;
        }
        let r = &mut self.cells[row];
        let remove_count = n.min(self.cols.saturating_sub(col));
        for _ in 0..remove_count {
            r.remove(col);
            r.push(Cell::default());
        }
        self.dirty_rows.insert(row);
    }

    // -- scroll region ------------------------------------------------------

    /// Set the scrolling region to rows `top..=bottom` (0-based, inclusive).
    /// Cursor moves to (0, 0) afterwards.  Marks all rows dirty.
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        }
        self.cursor.row = 0;
        self.cursor.col = 0;
        self.mark_all_dirty();
    }

    /// Scroll the content within the scroll region up by `n` lines.
    ///
    /// Lines that scroll off the top of a full-screen scroll region go into
    /// scrollback.
    pub fn scroll_up(&mut self, n: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let n = n.min(bottom - top + 1);

        let is_full_screen = top == 0 && bottom == self.rows.saturating_sub(1);

        for _ in 0..n {
            let line = self.cells.remove(top);
            if is_full_screen {
                self.scrollback.push(line);
            }
            self.cells.insert(bottom, blank_row(self.cols));
        }

        if is_full_screen {
            trim_scrollback(&mut self.scrollback, self.max_scrollback);
        }

        for r in top..=bottom {
            self.dirty_rows.insert(r);
        }
    }

    /// Scroll the content within the scroll region down by `n` lines.
    pub fn scroll_down(&mut self, n: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let n = n.min(bottom - top + 1);

        for _ in 0..n {
            self.cells.remove(bottom);
            self.cells.insert(top, blank_row(self.cols));
        }

        for r in top..=bottom {
            self.dirty_rows.insert(r);
        }
    }

    // -- insert / delete lines ----------------------------------------------

    /// Insert `n` blank lines at the cursor row, scrolling existing lines
    /// within the scroll region downward.
    pub fn insert_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }

        let bottom = self.scroll_bottom;
        let n = n.min(bottom - row + 1);

        debug_assert!(bottom < self.cells.len());
        for _ in 0..n {
            self.cells.remove(bottom);
            self.cells.insert(row, blank_row(self.cols));
        }

        for r in row..=bottom {
            self.dirty_rows.insert(r);
        }
    }

    /// Delete `n` lines at the cursor row, scrolling lines below upward
    /// within the scroll region.
    pub fn delete_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }

        let bottom = self.scroll_bottom;
        let n = n.min(bottom - row + 1);

        debug_assert!(row < self.cells.len());
        for _ in 0..n {
            self.cells.remove(row);
            self.cells.insert(bottom, blank_row(self.cols));
        }

        for r in row..=bottom {
            self.dirty_rows.insert(r);
        }
    }

    // -- cursor save / restore (DECSC / DECRC) ------------------------------

    /// Save cursor position and current pen state.
    pub fn save_cursor(&mut self) {
        self.cursor.saved_row = self.cursor.row;
        self.cursor.saved_col = self.cursor.col;
        self.cursor.saved_fg = self.current_fg;
        self.cursor.saved_bg = self.current_bg;
        self.cursor.saved_attrs = self.current_attrs;
    }

    /// Restore previously saved cursor position and pen state.
    pub fn restore_cursor(&mut self) {
        self.cursor.row = self.cursor.saved_row.min(self.rows.saturating_sub(1));
        self.cursor.col = self.cursor.saved_col.min(self.cols.saturating_sub(1));
        self.current_fg = self.cursor.saved_fg;
        self.current_bg = self.cursor.saved_bg;
        self.current_attrs = self.cursor.saved_attrs;
    }

    // -- reset --------------------------------------------------------------

    /// Full terminal reset — clear grid, scrollback, cursor, modes, pen, and
    /// title.
    pub fn reset(&mut self) {
        self.cells = blank_grid(self.cols, self.rows);
        self.scrollback.clear();
        self.cursor = CursorState::default();
        self.modes = TerminalModes::default();
        self.current_fg = Color::Default;
        self.current_bg = Color::Default;
        self.current_attrs = CellAttrs::default();
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.tab_stops = default_tab_stops(self.cols);
        self.title.clear();
        self.response_bytes.clear();
        self.mark_all_dirty();
    }

    // -- dirty tracking -----------------------------------------------------

    /// Take and return the set of dirty row indices, leaving it empty.
    pub fn clear_dirty(&mut self) -> HashSet<usize> {
        std::mem::take(&mut self.dirty_rows)
    }

    /// Mark every visible row as dirty.
    pub fn mark_all_dirty(&mut self) {
        self.dirty_rows.clear();
        for r in 0..self.rows {
            self.dirty_rows.insert(r);
        }
    }

    // -- rendering helper ---------------------------------------------------

    /// Collapse consecutive cells on a row into styled spans.
    ///
    /// Adjacent cells with identical fg / bg / attrs are merged into a single
    /// [`StyledSpan`].
    pub fn row_to_styled_spans(&self, row: usize) -> Vec<StyledSpan> {
        if row >= self.rows {
            return Vec::new();
        }

        let line = &self.cells[row];
        let mut spans: Vec<StyledSpan> = Vec::new();

        for cell in line {
            match spans.last_mut() {
                Some(span) if span.fg == cell.fg && span.bg == cell.bg && span.attrs == cell.attrs => {
                    span.text.push(cell.c);
                }
                _ => {
                    spans.push(StyledSpan {
                        text: String::from(cell.c),
                        fg: cell.fg,
                        bg: cell.bg,
                        attrs: cell.attrs,
                    });
                }
            }
        }

        spans
    }

    // -- private helpers ----------------------------------------------------

    /// Set cells in `row` from `start..end` to the default blank cell and
    /// mark the row dirty.
    fn erase_cells_range(&mut self, row: usize, start: usize, end: usize) {
        if row >= self.rows {
            return;
        }
        let end = end.min(self.cols);
        for col in start..end {
            self.cells[row][col] = Cell::default();
        }
        self.dirty_rows.insert(row);
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Create a fresh `cols`-wide blank row.
fn blank_row(cols: usize) -> Vec<Cell> {
    vec![Cell::default(); cols]
}

/// Create a blank `cols x rows` grid.
fn blank_grid(cols: usize, rows: usize) -> Vec<Vec<Cell>> {
    (0..rows).map(|_| blank_row(cols)).collect()
}

/// Build the default tab-stop vector — every 8th column starting at column 8.
fn default_tab_stops(cols: usize) -> Vec<bool> {
    (0..cols).map(|i| i > 0 && i % 8 == 0).collect()
}

/// Trim scrollback to at most `max` lines, removing the oldest entries.
fn trim_scrollback(scrollback: &mut Vec<Vec<Cell>>, max: usize) {
    if scrollback.len() > max {
        let excess = scrollback.len() - max;
        scrollback.drain(0..excess);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_grid_dimensions() {
        let g = Grid::new(80, 24);
        assert_eq!(g.cols, 80);
        assert_eq!(g.rows, 24);
        assert_eq!(g.cells.len(), 24);
        assert_eq!(g.cells[0].len(), 80);
    }

    #[test]
    fn default_cell_is_space() {
        let c = Cell::default();
        assert_eq!(c.c, ' ');
        assert_eq!(c.fg, Color::Default);
        assert_eq!(c.bg, Color::Default);
        assert!(c.attrs.is_empty());
    }

    #[test]
    fn cell_attrs_bitflags() {
        let mut a = CellAttrs::default();
        assert!(a.is_empty());
        a.insert(CellAttrs::BOLD);
        assert!(a.contains(CellAttrs::BOLD));
        assert!(!a.contains(CellAttrs::ITALIC));
        a.insert(CellAttrs::ITALIC);
        assert!(a.contains(CellAttrs::BOLD));
        assert!(a.contains(CellAttrs::ITALIC));
        a.remove(CellAttrs::BOLD);
        assert!(!a.contains(CellAttrs::BOLD));
        assert!(a.contains(CellAttrs::ITALIC));
    }

    #[test]
    fn put_char_basic() {
        let mut g = Grid::new(80, 24);
        g.put_char('A');
        assert_eq!(g.cells[0][0].c, 'A');
        assert_eq!(g.cursor.col, 1);
    }

    #[test]
    fn auto_wrap() {
        let mut g = Grid::new(3, 2);
        g.put_char('A');
        g.put_char('B');
        g.put_char('C'); // fills col 2, cursor now at col 3 (past last)
        g.put_char('D'); // should wrap to row 1 col 0, then write D at (1,0)
        assert_eq!(g.cells[0][0].c, 'A');
        assert_eq!(g.cells[0][1].c, 'B');
        assert_eq!(g.cells[0][2].c, 'C');
        assert_eq!(g.cells[1][0].c, 'D');
        assert_eq!(g.cursor.row, 1);
        assert_eq!(g.cursor.col, 1);
    }

    #[test]
    fn scroll_up_moves_to_scrollback() {
        let mut g = Grid::new(5, 2);
        g.put_char('A');
        g.cursor_to(1, 0);
        g.put_char('B');
        // Now scroll up — row 0 ("A    ") goes to scrollback.
        g.scroll_up(1);
        assert_eq!(g.scrollback.len(), 1);
        assert_eq!(g.scrollback[0][0].c, 'A');
        // Row 0 of visible grid should now be the old row 1.
        assert_eq!(g.cells[0][0].c, 'B');
    }

    #[test]
    fn tab_stops_every_8() {
        let g = Grid::new(80, 24);
        assert!(!g.tab_stops[0]);
        assert!(g.tab_stops[8]);
        assert!(g.tab_stops[16]);
        assert!(!g.tab_stops[7]);
    }

    #[test]
    fn tab_advances_to_next_stop() {
        let mut g = Grid::new(80, 24);
        g.cursor.col = 0;
        g.tab();
        assert_eq!(g.cursor.col, 8);
        g.tab();
        assert_eq!(g.cursor.col, 16);
    }

    #[test]
    fn erase_display_cursor_to_end() {
        let mut g = Grid::new(5, 3);
        for c in "HELLO".chars() {
            g.put_char(c);
        }
        g.cursor_to(1, 0);
        for c in "WORLD".chars() {
            g.put_char(c);
        }
        g.cursor_to(2, 0);
        for c in "ABCDE".chars() {
            g.put_char(c);
        }
        // Place cursor at row 1, col 2 and erase from cursor to end.
        g.cursor_to(1, 2);
        g.erase_display(0);
        // Row 0 should be intact.
        assert_eq!(g.cells[0][0].c, 'H');
        // Row 1 cols 0-1 intact, col 2+ erased.
        assert_eq!(g.cells[1][0].c, 'W');
        assert_eq!(g.cells[1][1].c, 'O');
        assert_eq!(g.cells[1][2].c, ' ');
        // Row 2 fully erased.
        assert_eq!(g.cells[2][0].c, ' ');
    }

    #[test]
    fn resize_preserves_content() {
        let mut g = Grid::new(5, 2);
        g.put_char('X');
        g.resize(10, 3);
        assert_eq!(g.cols, 10);
        assert_eq!(g.rows, 3);
        assert_eq!(g.cells[0][0].c, 'X');
    }

    #[test]
    fn save_restore_cursor() {
        let mut g = Grid::new(80, 24);
        g.cursor_to(5, 10);
        g.current_fg = Color::Indexed(1);
        g.save_cursor();
        g.cursor_to(0, 0);
        g.current_fg = Color::Default;
        g.restore_cursor();
        assert_eq!(g.cursor.row, 5);
        assert_eq!(g.cursor.col, 10);
        assert_eq!(g.current_fg, Color::Indexed(1));
    }

    #[test]
    fn row_to_styled_spans_merges() {
        let mut g = Grid::new(5, 1);
        // Write "ABC" with default pen — should become one span.
        g.put_char('A');
        g.put_char('B');
        g.put_char('C');
        let spans = g.row_to_styled_spans(0);
        // "ABC" + two trailing spaces — same style → one span.
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "ABC  ");
    }

    #[test]
    fn row_to_styled_spans_splits_on_color() {
        let mut g = Grid::new(4, 1);
        g.put_char('A');
        g.current_fg = Color::Indexed(1);
        g.put_char('B');
        g.put_char('C');
        let spans = g.row_to_styled_spans(0);
        // "A" (default) | "BC" (red) | " " (default, from Cell::default)
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].text, "A");
        assert_eq!(spans[1].text, "BC");
        assert_eq!(spans[2].text, " ");
    }

    #[test]
    fn insert_mode_shifts_cells() {
        let mut g = Grid::new(5, 1);
        for c in "ABCD".chars() {
            g.put_char(c);
        }
        // Turn on insert mode, go back, and insert.
        g.modes.insert_mode = true;
        g.cursor_to(0, 1);
        g.put_char('X');
        // Row should be: A X B C D  (the original 'D' at col 3 pushed to col 4,
        // nothing is lost because the row was only 4 wide + 1 trailing space).
        assert_eq!(g.cells[0][0].c, 'A');
        assert_eq!(g.cells[0][1].c, 'X');
        assert_eq!(g.cells[0][2].c, 'B');
        assert_eq!(g.cells[0][3].c, 'C');
        assert_eq!(g.cells[0][4].c, 'D');
    }

    #[test]
    fn delete_chars_shifts_left() {
        let mut g = Grid::new(5, 1);
        for c in "ABCDE".chars() {
            g.put_char(c);
        }
        g.cursor_to(0, 1);
        g.delete_chars(2);
        assert_eq!(g.cells[0][0].c, 'A');
        assert_eq!(g.cells[0][1].c, 'D');
        assert_eq!(g.cells[0][2].c, 'E');
        assert_eq!(g.cells[0][3].c, ' ');
        assert_eq!(g.cells[0][4].c, ' ');
    }

    #[test]
    fn clear_dirty_empties_set() {
        let mut g = Grid::new(80, 24);
        assert!(!g.dirty_rows.is_empty());
        let dirty = g.clear_dirty();
        assert_eq!(dirty.len(), 24);
        assert!(g.dirty_rows.is_empty());
    }

    #[test]
    fn scroll_region_insert_delete_lines() {
        let mut g = Grid::new(3, 5);
        // Fill rows with identifiable chars.
        for r in 0..5u8 {
            g.cursor_to(r as usize, 0);
            g.put_char((b'A' + r) as char);
        }
        // Set scroll region to rows 1..3.
        g.set_scroll_region(1, 3);
        g.cursor_to(1, 0);
        g.insert_lines(1);
        // Row 0 unchanged.
        assert_eq!(g.cells[0][0].c, 'A');
        // Row 1 should be blank (inserted).
        assert_eq!(g.cells[1][0].c, ' ');
        // Old row 1 ('B') should now be at row 2.
        assert_eq!(g.cells[2][0].c, 'B');
        // Old row 2 ('C') should now be at row 3 (old row 3 'D' scrolled out).
        assert_eq!(g.cells[3][0].c, 'C');
        // Row 4 is outside scroll region, unchanged.
        assert_eq!(g.cells[4][0].c, 'E');
    }

    #[test]
    fn reset_clears_everything() {
        let mut g = Grid::new(10, 5);
        g.put_char('Z');
        g.current_fg = Color::Rgb(255, 0, 0);
        g.title = "test".into();
        g.reset();
        assert_eq!(g.cells[0][0].c, ' ');
        assert_eq!(g.current_fg, Color::Default);
        assert!(g.title.is_empty());
        assert!(g.scrollback.is_empty());
    }
}
