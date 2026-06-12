// SPDX-License-Identifier: AGPL-3.0-or-later

//! Maps VT escape sequences to [`Grid`] mutations via the [`vte::Perform`]
//! trait.  Feed bytes from a PTY into a [`vte::Parser`] with a
//! [`TerminalHandler`] and the grid will be updated accordingly.

use super::grid::{CellAttrs, Color, Grid};

/// Thin adapter that translates VTE callbacks into [`Grid`] mutations.
pub struct TerminalHandler<'a> {
    grid: &'a mut Grid,
}

impl<'a> TerminalHandler<'a> {
    pub fn new(grid: &'a mut Grid) -> Self {
        Self { grid }
    }
}

// ---------------------------------------------------------------------------
// vte::Perform
// ---------------------------------------------------------------------------

impl vte::Perform for TerminalHandler<'_> {
    // -- printable character ------------------------------------------------

    fn print(&mut self, c: char) {
        self.grid.put_char(c);
    }

    // -- C0 control characters ----------------------------------------------

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => { /* BEL — ignored (visual bell not implemented) */ }
            0x08 => self.grid.backspace(),
            0x09 => self.grid.tab(),
            0x0A..=0x0C => self.grid.newline(),
            0x0D => self.grid.carriage_return(),
            _ => {}
        }
    }

    // -- CSI sequences ------------------------------------------------------

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // Flatten params into a simple Vec<u16> for ergonomic indexing.
        // Each sub-param slice is a semicolon-separated group; we take the
        // first element of each group.
        // TODO: colon-style 38:2::r:g:b subparam encoding not yet handled
        let params: Vec<u16> = params.iter().map(|sub| sub[0]).collect();

        /// Return `params[idx]` or `default` when absent / zero.
        fn p(params: &[u16], idx: usize, default: u16) -> u16 {
            params.get(idx).copied().filter(|&v| v != 0).unwrap_or(default)
        }

        match action {
            // -- cursor movement --------------------------------------------
            'A' => self.grid.cursor_up(p(&params, 0, 1) as usize),
            'B' => self.grid.cursor_down(p(&params, 0, 1) as usize),
            'C' => self.grid.cursor_forward(p(&params, 0, 1) as usize),
            'D' => self.grid.cursor_back(p(&params, 0, 1) as usize),
            'E' => {
                let n = p(&params, 0, 1) as usize;
                self.grid.cursor_down(n);
                self.grid.carriage_return();
            }
            'F' => {
                let n = p(&params, 0, 1) as usize;
                self.grid.cursor_up(n);
                self.grid.carriage_return();
            }
            'G' => {
                let col = p(&params, 0, 1) as usize;
                self.grid.cursor_to(self.grid.cursor.row, col.saturating_sub(1));
            }
            'H' | 'f' => {
                let row = p(&params, 0, 1) as usize;
                let col = p(&params, 1, 1) as usize;
                let origin = if self.grid.modes.origin_mode { self.grid.scroll_top } else { 0 };
                self.grid.cursor_to(row.saturating_sub(1) + origin, col.saturating_sub(1));
            }
            'd' => {
                let row = p(&params, 0, 1) as usize;
                let origin = if self.grid.modes.origin_mode { self.grid.scroll_top } else { 0 };
                self.grid.cursor_to(row.saturating_sub(1) + origin, self.grid.cursor.col);
            }

            // -- erase ------------------------------------------------------
            'J' => {
                let mode = params.first().copied().unwrap_or(0);
                self.grid.erase_display(mode);
            }
            'K' => {
                let mode = params.first().copied().unwrap_or(0);
                self.grid.erase_line(mode);
            }

            // -- SGR (Select Graphic Rendition) -----------------------------
            'm' => self.handle_sgr(&params),

            // -- scroll region and scrolling --------------------------------
            'r' => {
                let top = p(&params, 0, 1) as usize;
                let bottom = p(&params, 1, self.grid.rows as u16) as usize;
                self.grid.set_scroll_region(
                    top.saturating_sub(1),
                    bottom.saturating_sub(1),
                );
            }
            'S' => self.grid.scroll_up(p(&params, 0, 1) as usize),
            'T' => self.grid.scroll_down(p(&params, 0, 1) as usize),

            // -- mode set / reset ------------------------------------------
            'h' => self.handle_mode_set(&params, intermediates, true),
            'l' => self.handle_mode_set(&params, intermediates, false),

            // -- insert / delete characters ---------------------------------
            '@' => self.grid.insert_chars(p(&params, 0, 1) as usize),
            'P' => self.grid.delete_chars(p(&params, 0, 1) as usize),
            'X' => self.grid.erase_chars(p(&params, 0, 1) as usize),
            'L' => self.grid.insert_lines(p(&params, 0, 1) as usize),
            'M' => self.grid.delete_lines(p(&params, 0, 1) as usize),

            // -- cursor save / restore (ANSI) -------------------------------
            's' => self.grid.save_cursor(),
            'u' => self.grid.restore_cursor(),

            // -- device status report ---------------------------------------
            'n' if params.first().copied() == Some(6) => {
                let row = self.grid.cursor.row + 1;
                let col = self.grid.cursor.col + 1;
                let response = format!("\x1b[{row};{col}R");
                self.grid.response_bytes.extend_from_slice(response.as_bytes());
            }

            // -- tab clear --------------------------------------------------
            'g' => {
                let mode = params.first().copied().unwrap_or(0);
                match mode {
                    0 => {
                        let col = self.grid.cursor.col;
                        if col < self.grid.tab_stops.len() {
                            self.grid.tab_stops[col] = false;
                        }
                    }
                    3 => {
                        for stop in &mut self.grid.tab_stops {
                            *stop = false;
                        }
                    }
                    _ => {}
                }
            }

            _ => {}
        }
    }

    // -- OSC sequences ------------------------------------------------------

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }
        // OSC 0 (icon name + title) and OSC 2 (title) both set the title.
        match params[0] {
            b"0" | b"2" => {
                if let Some(title_bytes) = params.get(1) {
                    self.grid.title = String::from_utf8_lossy(title_bytes).into_owned();
                }
            }
            _ => {}
        }
    }

    // -- ESC sequences ------------------------------------------------------

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates, byte) {
            (_, b'7') => self.grid.save_cursor(),
            (_, b'8') => self.grid.restore_cursor(),
            (_, b'c') => self.grid.reset(),
            // Charset designation (e.g. ESC ( B) — ignored.
            ([b'(' | b')'], _) => {}
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

impl TerminalHandler<'_> {
    /// Parse SGR (Select Graphic Rendition) parameters and apply them to the
    /// grid's current pen state.
    fn handle_sgr(&mut self, params: &[u16]) {
        // An empty param list is equivalent to SGR 0 (reset).
        if params.is_empty() {
            self.grid.current_fg = Color::Default;
            self.grid.current_bg = Color::Default;
            self.grid.current_attrs = CellAttrs::default();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                // -- reset --------------------------------------------------
                0 => {
                    self.grid.current_fg = Color::Default;
                    self.grid.current_bg = Color::Default;
                    self.grid.current_attrs = CellAttrs::default();
                }

                // -- set attributes -----------------------------------------
                1 => self.grid.current_attrs.insert(CellAttrs::BOLD),
                2 => self.grid.current_attrs.insert(CellAttrs::DIM),
                3 => self.grid.current_attrs.insert(CellAttrs::ITALIC),
                4 => self.grid.current_attrs.insert(CellAttrs::UNDERLINE),
                7 => self.grid.current_attrs.insert(CellAttrs::INVERSE),
                9 => self.grid.current_attrs.insert(CellAttrs::STRIKETHROUGH),

                // -- reset attributes ---------------------------------------
                22 => {
                    self.grid.current_attrs.remove(CellAttrs::BOLD);
                    self.grid.current_attrs.remove(CellAttrs::DIM);
                }
                23 => self.grid.current_attrs.remove(CellAttrs::ITALIC),
                24 => self.grid.current_attrs.remove(CellAttrs::UNDERLINE),
                27 => self.grid.current_attrs.remove(CellAttrs::INVERSE),
                29 => self.grid.current_attrs.remove(CellAttrs::STRIKETHROUGH),

                // -- standard foreground colors (30–37) ---------------------
                30..=37 => {
                    self.grid.current_fg = Color::Indexed((params[i] - 30) as u8);
                }

                // -- extended foreground (38;5;n or 38;2;r;g;b) -------------
                38 => {
                    i += 1;
                    if i >= params.len() {
                        break;
                    }
                    match params[i] {
                        5 if i + 1 < params.len() => {
                            i += 1;
                            self.grid.current_fg = Color::Indexed(params[i] as u8);
                        }
                        2 if i + 3 < params.len() => {
                            let r = params[i + 1] as u8;
                            let g = params[i + 2] as u8;
                            let b = params[i + 3] as u8;
                            self.grid.current_fg = Color::Rgb(r, g, b);
                            i += 3;
                        }
                        _ => {}
                    }
                }

                // -- default foreground -------------------------------------
                39 => self.grid.current_fg = Color::Default,

                // -- standard background colors (40–47) ---------------------
                40..=47 => {
                    self.grid.current_bg = Color::Indexed((params[i] - 40) as u8);
                }

                // -- extended background (48;5;n or 48;2;r;g;b) -------------
                48 => {
                    i += 1;
                    if i >= params.len() {
                        break;
                    }
                    match params[i] {
                        5 if i + 1 < params.len() => {
                            i += 1;
                            self.grid.current_bg = Color::Indexed(params[i] as u8);
                        }
                        2 if i + 3 < params.len() => {
                            let r = params[i + 1] as u8;
                            let g = params[i + 2] as u8;
                            let b = params[i + 3] as u8;
                            self.grid.current_bg = Color::Rgb(r, g, b);
                            i += 3;
                        }
                        _ => {}
                    }
                }

                // -- default background -------------------------------------
                49 => self.grid.current_bg = Color::Default,

                // -- bright foreground colors (90–97) -----------------------
                90..=97 => {
                    self.grid.current_fg = Color::Indexed((params[i] - 90 + 8) as u8);
                }

                // -- bright background colors (100–107) ---------------------
                100..=107 => {
                    self.grid.current_bg = Color::Indexed((params[i] - 100 + 8) as u8);
                }

                _ => {}
            }
            i += 1;
        }
    }

    /// Handle SM/DECSET ('h') and RM/DECRST ('l') mode commands.
    fn handle_mode_set(&mut self, params: &[u16], intermediates: &[u8], enable: bool) {
        let is_dec_private = intermediates.contains(&b'?');

        for &param in params {
            if is_dec_private {
                match param {
                    1 => self.grid.modes.application_cursor_keys = enable,
                    6 => self.grid.modes.origin_mode = enable,
                    7 => self.grid.modes.auto_wrap = enable,
                    25 => self.grid.modes.cursor_visible = enable,
                    47 => self.grid.modes.alternate_screen = enable,
                    1049 => {
                        if enable {
                            self.grid.save_cursor();
                            self.grid.modes.alternate_screen = true;
                            self.grid.erase_display(2);
                        } else {
                            self.grid.modes.alternate_screen = false;
                            self.grid.erase_display(2);
                            self.grid.restore_cursor();
                        }
                    }
                    2004 => self.grid.modes.bracketed_paste = enable,
                    _ => {}
                }
            } else if param == 4 {
                self.grid.modes.insert_mode = enable;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed raw bytes through a VTE parser + our handler and return the grid.
    fn parse(cols: usize, rows: usize, input: &[u8]) -> Grid {
        let mut grid = Grid::new(cols, rows);
        let mut parser = vte::Parser::new();
        for &byte in input {
            let mut handler = TerminalHandler::new(&mut grid);
            parser.advance(&mut handler, byte);
        }
        grid
    }

    #[test]
    fn print_basic_text() {
        let grid = parse(10, 1, b"Hello");
        assert_eq!(grid.cells[0][0].c, 'H');
        assert_eq!(grid.cells[0][4].c, 'o');
        assert_eq!(grid.cursor.col, 5);
    }

    #[test]
    fn carriage_return_and_newline() {
        let grid = parse(10, 3, b"AB\r\nCD");
        assert_eq!(grid.cells[0][0].c, 'A');
        assert_eq!(grid.cells[0][1].c, 'B');
        assert_eq!(grid.cells[1][0].c, 'C');
        assert_eq!(grid.cells[1][1].c, 'D');
    }

    #[test]
    fn backspace() {
        let grid = parse(10, 1, b"AB\x08X");
        // After "AB", backspace moves cursor from 2 to 1, then 'X' overwrites 'B'.
        assert_eq!(grid.cells[0][0].c, 'A');
        assert_eq!(grid.cells[0][1].c, 'X');
    }

    #[test]
    fn cursor_up_down() {
        // ESC[2B = cursor down 2, ESC[1A = cursor up 1
        let grid = parse(10, 5, b"\x1b[2B\x1b[1AX");
        // Down 2 → row 2, up 1 → row 1, print X at (1,0).
        assert_eq!(grid.cells[1][0].c, 'X');
    }

    #[test]
    fn cursor_absolute_position() {
        // ESC[3;5H = move to row 3, col 5 (1-based)
        let grid = parse(10, 5, b"\x1b[3;5HX");
        assert_eq!(grid.cells[2][4].c, 'X');
    }

    #[test]
    fn erase_display_from_cursor() {
        let grid = parse(5, 2, b"HELLO\r\nWORLD\x1b[1;3H\x1b[J");
        // Cursor at (0,2), erase from cursor to end.
        assert_eq!(grid.cells[0][0].c, 'H');
        assert_eq!(grid.cells[0][1].c, 'E');
        assert_eq!(grid.cells[0][2].c, ' '); // erased
        assert_eq!(grid.cells[1][0].c, ' '); // erased
    }

    #[test]
    fn sgr_bold_and_color() {
        // ESC[1;31m = bold + red foreground, then print, then ESC[0m = reset
        let grid = parse(10, 1, b"\x1b[1;31mA\x1b[0mB");
        assert!(grid.cells[0][0].attrs.contains(CellAttrs::BOLD));
        assert_eq!(grid.cells[0][0].fg, Color::Indexed(1));
        // After reset:
        assert!(grid.cells[0][1].attrs.is_empty());
        assert_eq!(grid.cells[0][1].fg, Color::Default);
    }

    #[test]
    fn sgr_256_color() {
        // ESC[38;5;214m = fg color 214
        let grid = parse(10, 1, b"\x1b[38;5;214mX");
        assert_eq!(grid.cells[0][0].fg, Color::Indexed(214));
    }

    #[test]
    fn sgr_rgb_color() {
        // ESC[38;2;100;150;200m = fg RGB(100,150,200)
        let grid = parse(10, 1, b"\x1b[38;2;100;150;200mX");
        assert_eq!(grid.cells[0][0].fg, Color::Rgb(100, 150, 200));
    }

    #[test]
    fn sgr_bright_colors() {
        // ESC[91m = bright red fg (indexed 9)
        let grid = parse(10, 1, b"\x1b[91mX");
        assert_eq!(grid.cells[0][0].fg, Color::Indexed(9));
    }

    #[test]
    fn osc_set_title() {
        // OSC 2 ; title BEL
        let grid = parse(10, 1, b"\x1b]2;My Terminal\x07");
        assert_eq!(grid.title, "My Terminal");
    }

    #[test]
    fn scroll_region() {
        // DECSTBM: ESC[2;4r — set scroll region rows 2-4 (1-based)
        let grid = parse(5, 5, b"\x1b[2;4r");
        assert_eq!(grid.scroll_top, 1);
        assert_eq!(grid.scroll_bottom, 3);
    }

    #[test]
    fn mode_set_and_reset() {
        // DECSET ?25 (show cursor), then DECRST ?25 (hide cursor)
        let mut grid = Grid::new(10, 5);
        let mut parser = vte::Parser::new();

        // Hide cursor: ESC[?25l
        for &byte in b"\x1b[?25l" {
            let mut handler = TerminalHandler::new(&mut grid);
            parser.advance(&mut handler, byte);
        }
        assert!(!grid.modes.cursor_visible);

        // Show cursor: ESC[?25h
        for &byte in b"\x1b[?25h" {
            let mut handler = TerminalHandler::new(&mut grid);
            parser.advance(&mut handler, byte);
        }
        assert!(grid.modes.cursor_visible);
    }

    #[test]
    fn device_status_report() {
        // Position cursor at (2,4), then DSR: ESC[6n
        let grid = parse(10, 5, b"\x1b[3;5H\x1b[6n");
        // Response should be ESC[3;5R (1-based)
        assert_eq!(grid.response_bytes, b"\x1b[3;5R");
    }

    #[test]
    fn esc_save_restore_cursor() {
        // ESC 7 = save, move, ESC 8 = restore
        let grid = parse(10, 5, b"AB\x1b7\x1b[5;1H\x1b8X");
        // After "AB" cursor is at (0,2). Save. Move to (4,0). Restore → (0,2).
        // Print X at (0,2).
        assert_eq!(grid.cells[0][2].c, 'X');
    }

    #[test]
    fn esc_full_reset() {
        let mut grid = Grid::new(10, 5);
        grid.put_char('Z');
        grid.current_fg = Color::Indexed(3);
        grid.title = "test".into();

        let mut parser = vte::Parser::new();
        for &byte in b"\x1bc" {
            let mut handler = TerminalHandler::new(&mut grid);
            parser.advance(&mut handler, byte);
        }

        assert_eq!(grid.cells[0][0].c, ' ');
        assert_eq!(grid.current_fg, Color::Default);
        assert!(grid.title.is_empty());
    }

    #[test]
    fn insert_and_delete_chars() {
        // Write "ABCDE", move to col 1, insert 2 blanks
        let grid = parse(5, 1, b"ABCDE\x1b[1;2H\x1b[2@");
        assert_eq!(grid.cells[0][0].c, 'A');
        assert_eq!(grid.cells[0][1].c, ' ');
        assert_eq!(grid.cells[0][2].c, ' ');
        assert_eq!(grid.cells[0][3].c, 'B');
        assert_eq!(grid.cells[0][4].c, 'C');
    }

    #[test]
    fn tab_clear() {
        let mut grid = Grid::new(80, 24);
        assert!(grid.tab_stops[8]); // default stop at col 8

        let mut parser = vte::Parser::new();
        // Move to col 8, then clear current tab stop: ESC[0g
        for &byte in b"\x1b[9G\x1b[0g" {
            let mut handler = TerminalHandler::new(&mut grid);
            parser.advance(&mut handler, byte);
        }
        assert!(!grid.tab_stops[8]);

        // Clear all tab stops: ESC[3g
        for &byte in b"\x1b[3g" {
            let mut handler = TerminalHandler::new(&mut grid);
            parser.advance(&mut handler, byte);
        }
        for stop in &grid.tab_stops {
            assert!(!stop);
        }
    }

    #[test]
    fn vpa_absolute_row() {
        // ESC[3d = move to row 3 (1-based), keeping current column
        let grid = parse(10, 5, b"\x1b[1;5H\x1b[3d");
        assert_eq!(grid.cursor.row, 2);
        assert_eq!(grid.cursor.col, 4); // column unchanged
    }

    #[test]
    fn cnl_and_cpl() {
        // CNL: ESC[2E = cursor down 2 + carriage return
        let grid = parse(10, 5, b"\x1b[1;5H\x1b[2E");
        assert_eq!(grid.cursor.row, 2);
        assert_eq!(grid.cursor.col, 0);

        // CPL: ESC[1F = cursor up 1 + carriage return
        let grid = parse(10, 5, b"\x1b[3;5H\x1b[1F");
        assert_eq!(grid.cursor.row, 1);
        assert_eq!(grid.cursor.col, 0);
    }
}
