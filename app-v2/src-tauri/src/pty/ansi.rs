use std::path::Path;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollbackKind {
    Prompt,
    Cmd,
    Dim,
    Ok,
    Err,
    File,
    Str,
    Ai,
    Path,
    Ask,
    Plain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollbackLine {
    pub kind: ScrollbackKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
struct TermDimensions {
    cols: usize,
    rows: usize,
}

impl TermDimensions {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        }
    }
}

impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeState {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

pub struct AnsiLineDecoder {
    dims: TermDimensions,
    parser: Processor,
    term: Term<VoidListener>,
    state: EscapeState,
    current: String,
    saw_cr: bool,
    utf8: Vec<u8>,
}

impl AnsiLineDecoder {
    pub fn new(cols: u16, rows: u16) -> Self {
        let dims = TermDimensions::new(cols, rows);
        let term = Term::new(TermConfig::default(), &dims, VoidListener);
        Self {
            dims,
            parser: Processor::new(),
            term,
            state: EscapeState::Ground,
            current: String::new(),
            saw_cr: false,
            utf8: Vec::new(),
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.dims = TermDimensions::new(cols, rows);
        self.term.resize(self.dims);
    }

    pub fn reset(&mut self) {
        let dims = self.dims;
        self.parser = Processor::new();
        self.term = Term::new(TermConfig::default(), &dims, VoidListener);
        self.state = EscapeState::Ground;
        self.current.clear();
        self.saw_cr = false;
        self.utf8.clear();
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<ScrollbackLine> {
        let mut lines = Vec::new();

        for &byte in bytes {
            self.parser.advance(&mut self.term, &[byte]);

            if self.saw_cr {
                if byte == b'\n' {
                    lines.push(self.flush_current());
                    self.saw_cr = false;
                    continue;
                }
                self.current.clear();
                self.saw_cr = false;
            }

            match self.state {
                EscapeState::Ground => match byte {
                    0x1b => self.state = EscapeState::Escape,
                    b'\r' => self.saw_cr = true,
                    b'\n' => lines.push(self.flush_current()),
                    0x08 => {
                        self.flush_utf8_lossy();
                        self.current.pop();
                    }
                    0x00..=0x1f | 0x7f => {}
                    byte if byte.is_ascii() => {
                        self.flush_utf8_lossy();
                        self.current.push(byte as char);
                    }
                    byte => self.push_utf8(byte),
                },
                EscapeState::Escape => match byte {
                    b'[' => self.state = EscapeState::Csi,
                    b']' => self.state = EscapeState::Osc,
                    _ => self.state = EscapeState::Ground,
                },
                EscapeState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.state = EscapeState::Ground;
                    }
                }
                EscapeState::Osc => match byte {
                    0x07 => self.state = EscapeState::Ground,
                    0x1b => self.state = EscapeState::OscEscape,
                    _ => {}
                },
                EscapeState::OscEscape => {
                    self.state = if byte == b'\\' {
                        EscapeState::Ground
                    } else {
                        EscapeState::Osc
                    };
                }
            }
        }

        lines
    }

    fn push_utf8(&mut self, byte: u8) {
        self.utf8.push(byte);
        match std::str::from_utf8(&self.utf8) {
            Ok(text) => {
                self.current.push_str(text);
                self.utf8.clear();
            }
            Err(err) if err.error_len().is_none() => {}
            Err(_) => self.flush_utf8_lossy(),
        }
    }

    fn flush_utf8_lossy(&mut self) {
        if self.utf8.is_empty() {
            return;
        }
        self.current.push_str(&String::from_utf8_lossy(&self.utf8));
        self.utf8.clear();
    }

    fn flush_current(&mut self) -> ScrollbackLine {
        self.flush_utf8_lossy();
        let text = std::mem::take(&mut self.current);
        let kind = self.kind_for_flushed_line(&text);
        ScrollbackLine { kind, text }
    }

    fn kind_for_flushed_line(&self, text: &str) -> ScrollbackKind {
        if text.starts_with("↳ ask: ") {
            return ScrollbackKind::Ask;
        }

        let trimmed = text.trim();
        if trimmed.is_empty() {
            return ScrollbackKind::Plain;
        }

        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("last login")
            || lower == "logout"
            || lower.starts_with("on branch ")
            || lower.starts_with("your branch is up to date")
            || lower == "nothing to commit, working tree clean"
        {
            return ScrollbackKind::Dim;
        }

        if lower.contains("command not found")
            || lower.contains("no such file")
            || lower.contains("permission denied")
            || lower.starts_with("zsh:")
            || lower.starts_with("bash:")
            || lower.contains("error")
        {
            return ScrollbackKind::Err;
        }

        if is_probable_path(trimmed) {
            return ScrollbackKind::Path;
        }

        if lower.contains("claude code")
            || lower.contains("codex")
            || lower.contains("antigravity")
            || lower.contains("agy")
            || lower.contains("opencode")
        {
            return ScrollbackKind::Ai;
        }

        let row_kind = self.kind_from_latest_row();
        if row_kind != ScrollbackKind::Plain {
            return row_kind;
        }

        ScrollbackKind::Plain
    }

    fn kind_from_latest_row(&self) -> ScrollbackKind {
        let cursor_line = self.term.grid().cursor.point.line;
        let row_line = Line(cursor_line.0.saturating_sub(1));
        let row = &self.term.grid()[row_line];

        let mut saw_dim = false;
        let mut saw_italic = false;
        let mut saw_green = 0usize;
        let mut saw_red = 0usize;
        let mut saw_yellow = 0usize;
        let mut saw_blue = 0usize;

        for cell in row {
            if cell.c == ' ' && cell.zerowidth().is_none() {
                continue;
            }

            if cell.flags.contains(Flags::DIM) {
                saw_dim = true;
            }
            if cell.flags.contains(Flags::ITALIC) {
                saw_italic = true;
            }

            match cell.fg {
                Color::Named(NamedColor::Red)
                | Color::Named(NamedColor::BrightRed)
                | Color::Named(NamedColor::DimRed) => saw_red += 1,
                Color::Named(NamedColor::Green)
                | Color::Named(NamedColor::BrightGreen)
                | Color::Named(NamedColor::DimGreen) => saw_green += 1,
                Color::Named(NamedColor::Yellow)
                | Color::Named(NamedColor::BrightYellow)
                | Color::Named(NamedColor::DimYellow) => saw_yellow += 1,
                Color::Named(NamedColor::Blue)
                | Color::Named(NamedColor::BrightBlue)
                | Color::Named(NamedColor::Cyan)
                | Color::Named(NamedColor::BrightCyan)
                | Color::Named(NamedColor::DimBlue)
                | Color::Named(NamedColor::DimCyan) => saw_blue += 1,
                _ => {}
            }
        }

        if saw_red > 0 {
            return ScrollbackKind::Err;
        }
        if saw_dim {
            return ScrollbackKind::Dim;
        }
        if saw_italic {
            return ScrollbackKind::Ai;
        }
        if saw_yellow > 0 {
            return ScrollbackKind::File;
        }
        if saw_green > 0 {
            return ScrollbackKind::Prompt;
        }
        if saw_blue > 0 {
            return ScrollbackKind::Str;
        }

        ScrollbackKind::Plain
    }
}

fn is_probable_path(text: &str) -> bool {
    text.starts_with("~/")
        || text.starts_with('/')
        || text
            .split_whitespace()
            .all(|part| part.starts_with("./") || Path::new(part).is_absolute())
}

// ─────────────────────── Grid snapshot (alacritty Term → JSON cells) ───
//
// Honors I-4: agent / smart seats render the alacritty Term's grid
// directly. The `AnsiLineDecoder` already maintains a correct `Term`
// (it always did — we just ignored the grid and tried to slice lines
// from the raw byte stream, which broke for any TUI). `snapshot()`
// walks the visible grid and serialises it for the frontend.
//
// Payload size at typical 100×28 = 2800 cells × ~50 B JSON ≈ 140 KB
// per emit; on sub-second CC TUI churn this is ~1-2 MB/s which is
// bearable for MVP. Diff-based snapshots can be a follow-up if perf
// shows up as a problem.

/// One displayable grid cell. `ch` is the character (space for blank).
/// `fg` / `bg` are 0xRRGGBB; `None` means "use the renderer's default".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridCell {
    pub ch: String, // String not char — avoids JSON escape oddities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<u32>,
    /// Bitmask of attribute flags: 1=bold, 2=italic, 4=underline,
    /// 8=dim, 16=inverse, 32=strikeout.
    #[serde(skip_serializing_if = "is_zero")]
    #[serde(default)]
    pub attrs: u8,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

pub const ATTR_BOLD: u8 = 1 << 0;
pub const ATTR_ITALIC: u8 = 1 << 1;
pub const ATTR_UNDERLINE: u8 = 1 << 2;
pub const ATTR_DIM: u8 = 1 << 3;
pub const ATTR_INVERSE: u8 = 1 << 4;
pub const ATTR_STRIKEOUT: u8 = 1 << 5;
/// Cell holds a wide (CJK / fullwidth) glyph that occupies 2 grid columns.
/// The frontend uses this to force exact 2× cell-width rendering so the
/// browser monospace + CJK-fallback font's natural width drift can't
/// accumulate (~0.05 cell per char, drags cursor out of step over time).
pub const ATTR_WIDE: u8 = 1 << 6;

/// A snapshot of the visible terminal grid at a moment in time.
/// `cells` is row-major, length = `cols * rows`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<GridCell>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    #[serde(default)]
    pub display_offset: u32,
    #[serde(default)]
    pub mouse_mode: bool,
    #[serde(default)]
    pub sgr_mouse: bool,
}

impl AnsiLineDecoder {
    /// Scroll alacritty's display viewport. Positive `lines` moves up into
    /// older scrollback; negative moves back toward the live bottom.
    pub fn scroll_display(&mut self, lines: i32) {
        if lines == 0 {
            return;
        }
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// Walk the visible grid and produce a serialisable snapshot.
    pub fn snapshot(&self) -> GridSnapshot {
        let cols = self.dims.columns() as u16;
        let rows = self.dims.screen_lines() as u16;
        let mut cells = Vec::with_capacity(cols as usize * rows as usize);

        let grid = self.term.grid();
        let display_offset = grid.display_offset();
        for row_idx in 0..rows as i32 {
            let line = Line(row_idx - display_offset as i32);
            for col_idx in 0..cols as usize {
                let cell = &grid[line][Column(col_idx)];
                cells.push(cell_to_grid_cell(cell));
            }
        }

        let cursor = grid.cursor.point;
        let mode = self.term.mode();
        let cursor_visible = display_offset == 0 && mode.contains(TermMode::SHOW_CURSOR);

        GridSnapshot {
            cols,
            rows,
            cells,
            cursor_row: cursor.line.0.max(0) as u16,
            cursor_col: cursor.column.0 as u16,
            cursor_visible,
            display_offset: display_offset as u32,
            mouse_mode: mode.intersects(TermMode::MOUSE_MODE),
            sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
        }
    }
}

fn cell_to_grid_cell(cell: &alacritty_terminal::term::cell::Cell) -> GridCell {
    let mut attrs = 0u8;
    if cell.flags.contains(Flags::BOLD) {
        attrs |= ATTR_BOLD;
    }
    if cell.flags.contains(Flags::ITALIC) {
        attrs |= ATTR_ITALIC;
    }
    if cell.flags.intersects(Flags::ALL_UNDERLINES) {
        attrs |= ATTR_UNDERLINE;
    }
    if cell.flags.contains(Flags::DIM) {
        attrs |= ATTR_DIM;
    }
    if cell.flags.contains(Flags::INVERSE) {
        attrs |= ATTR_INVERSE;
    }
    if cell.flags.contains(Flags::STRIKEOUT) {
        attrs |= ATTR_STRIKEOUT;
    }
    if cell.flags.contains(Flags::WIDE_CHAR) {
        attrs |= ATTR_WIDE;
    }

    // CJK / wide-char layout: alacritty stores a wide glyph (e.g. 中) in
    // its left cell with WIDE_CHAR set, and reserves the right-hand cell
    // as a SPACER so the cursor advances by 2 columns. In a browser
    // monospace font the CJK glyph already paints across ~2 cell widths,
    // so the spacer must contribute zero — emit empty `ch`. Without this
    // the spacer's placeholder space adds a 3rd cell of visual width per
    // CJK char and the cursor (positioned by `cursorCol * cellWidth`)
    // drifts left into already-typed text.
    let ch = if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
    {
        String::new()
    } else if cell.c == '\0' {
        " ".to_string()
    } else {
        cell.c.to_string()
    };
    GridCell {
        ch,
        fg: color_to_rgb_u32(cell.fg, /*is_fg=*/ true),
        bg: color_to_rgb_u32(cell.bg, /*is_fg=*/ false),
        attrs,
    }
}

/// Map an alacritty `Color` to 0xRRGGBB. `None` = use default.
fn color_to_rgb_u32(color: Color, is_fg: bool) -> Option<u32> {
    match color {
        Color::Spec(rgb) => Some(rgb_pack(rgb)),
        Color::Indexed(idx) => indexed_to_rgb(idx).map(rgb_pack),
        Color::Named(named) => named_to_rgb(named, is_fg).map(rgb_pack),
    }
}

#[inline]
fn rgb_pack(rgb: Rgb) -> u32 {
    ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | (rgb.b as u32)
}

/// 256-color cube + grayscale ramp for `Color::Indexed(16..=255)`;
/// 0..=15 falls through to the named-color palette.
fn indexed_to_rgb(idx: u8) -> Option<Rgb> {
    if idx < 16 {
        return named_to_rgb(named_for_index(idx), true);
    }
    if (16..=231).contains(&idx) {
        let n = idx - 16;
        let r = ((n / 36) % 6) as u8;
        let g = ((n / 6) % 6) as u8;
        let b = (n % 6) as u8;
        let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return Some(Rgb {
            r: scale(r),
            g: scale(g),
            b: scale(b),
        });
    }
    if idx >= 232 {
        let v = 8 + (idx - 232) * 10;
        return Some(Rgb { r: v, g: v, b: v });
    }
    None
}

fn named_for_index(idx: u8) -> NamedColor {
    match idx {
        0 => NamedColor::Black,
        1 => NamedColor::Red,
        2 => NamedColor::Green,
        3 => NamedColor::Yellow,
        4 => NamedColor::Blue,
        5 => NamedColor::Magenta,
        6 => NamedColor::Cyan,
        7 => NamedColor::White,
        8 => NamedColor::BrightBlack,
        9 => NamedColor::BrightRed,
        10 => NamedColor::BrightGreen,
        11 => NamedColor::BrightYellow,
        12 => NamedColor::BrightBlue,
        13 => NamedColor::BrightMagenta,
        14 => NamedColor::BrightCyan,
        _ => NamedColor::BrightWhite,
    }
}

/// Default xterm-ish palette for the 16 ANSI colors plus Foreground /
/// Background / Cursor / Dim*. Returns `None` for "use renderer default"
/// (Foreground / Background — the React layer paints those via CSS).
fn named_to_rgb(named: NamedColor, _is_fg: bool) -> Option<Rgb> {
    let (r, g, b) = match named {
        NamedColor::Black => (0x00, 0x00, 0x00),
        NamedColor::Red => (0xCD, 0x31, 0x31),
        NamedColor::Green => (0x0D, 0xBC, 0x79),
        NamedColor::Yellow => (0xE5, 0xE5, 0x10),
        NamedColor::Blue => (0x24, 0x72, 0xC8),
        NamedColor::Magenta => (0xBC, 0x3F, 0xBC),
        NamedColor::Cyan => (0x11, 0xA8, 0xCD),
        NamedColor::White => (0xE5, 0xE5, 0xE5),
        NamedColor::BrightBlack => (0x66, 0x66, 0x66),
        NamedColor::BrightRed => (0xF1, 0x4C, 0x4C),
        NamedColor::BrightGreen => (0x23, 0xD1, 0x8B),
        NamedColor::BrightYellow => (0xF5, 0xF5, 0x43),
        NamedColor::BrightBlue => (0x3B, 0x8E, 0xEA),
        NamedColor::BrightMagenta => (0xD6, 0x70, 0xD6),
        NamedColor::BrightCyan => (0x29, 0xB8, 0xDB),
        NamedColor::BrightWhite => (0xFF, 0xFF, 0xFF),
        NamedColor::DimBlack => (0x00, 0x00, 0x00),
        NamedColor::DimRed => (0x86, 0x20, 0x20),
        NamedColor::DimGreen => (0x09, 0x7A, 0x4F),
        NamedColor::DimYellow => (0x95, 0x95, 0x0A),
        NamedColor::DimBlue => (0x18, 0x4A, 0x82),
        NamedColor::DimMagenta => (0x7A, 0x29, 0x7A),
        NamedColor::DimCyan => (0x0B, 0x6E, 0x86),
        NamedColor::DimWhite => (0x95, 0x95, 0x95),
        NamedColor::DimForeground => (0x99, 0x99, 0x99),
        NamedColor::Cursor => return None,
        // For Foreground / Background / BrightForeground let the CSS
        // theme decide — keeps the warm-Ghibli palette consistent.
        NamedColor::Foreground | NamedColor::Background | NamedColor::BrightForeground => {
            return None;
        }
    };
    Some(Rgb { r, g, b })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_basic_ansi_sequences() {
        let mut decoder = AnsiLineDecoder::new(80, 24);
        let lines = decoder.push_bytes(b"\x1b[31mboom\x1b[0m\r\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "boom");
        assert_eq!(lines[0].kind, ScrollbackKind::Err);
    }

    #[test]
    fn treats_carriage_return_without_newline_as_line_rewrite() {
        let mut decoder = AnsiLineDecoder::new(80, 24);
        let lines = decoder.push_bytes(b"first\rsecond\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "second");
    }

    #[test]
    fn resets_internal_state() {
        let mut decoder = AnsiLineDecoder::new(80, 24);
        let _ = decoder.push_bytes(b"hello\n");
        decoder.reset();
        let lines = decoder.push_bytes(b"world\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "world");
    }

    #[test]
    fn snapshot_returns_correct_dims() {
        let decoder = AnsiLineDecoder::new(20, 5);
        let snap = decoder.snapshot();
        assert_eq!(snap.cols, 20);
        assert_eq!(snap.rows, 5);
        assert_eq!(snap.cells.len(), 100);
        // Cursor starts at (0, 0).
        assert_eq!(snap.cursor_row, 0);
        assert_eq!(snap.cursor_col, 0);
    }

    #[test]
    fn snapshot_captures_plain_text() {
        let mut decoder = AnsiLineDecoder::new(20, 5);
        let _ = decoder.push_bytes(b"hello");
        let snap = decoder.snapshot();
        assert_eq!(snap.cells[0].ch, "h");
        assert_eq!(snap.cells[1].ch, "e");
        assert_eq!(snap.cells[2].ch, "l");
        assert_eq!(snap.cells[3].ch, "l");
        assert_eq!(snap.cells[4].ch, "o");
        // Cursor advances to col 5 of row 0.
        assert_eq!(snap.cursor_col, 5);
    }

    #[test]
    fn snapshot_respects_cursor_positioning_after_tui_sequence() {
        // CSI H = cursor home; CSI 2;5H = move to row 2, col 5; print "X".
        let mut decoder = AnsiLineDecoder::new(20, 5);
        let _ = decoder.push_bytes(b"\x1b[2;5HX");
        let snap = decoder.snapshot();
        // Row index 1 (zero-based), column 4 (zero-based) — that cell is "X".
        let cell_idx = 1 * snap.cols as usize + 4;
        assert_eq!(snap.cells[cell_idx].ch, "X");
    }

    #[test]
    fn snapshot_color_mapping() {
        let mut decoder = AnsiLineDecoder::new(20, 1);
        let _ = decoder.push_bytes(b"\x1b[31mR\x1b[0m");
        let snap = decoder.snapshot();
        // Red foreground should be 0xCD3131 per our default palette.
        assert_eq!(snap.cells[0].ch, "R");
        assert_eq!(snap.cells[0].fg, Some(0xCD3131));
    }

    #[test]
    fn snapshot_attrs_bits() {
        let mut decoder = AnsiLineDecoder::new(20, 1);
        let _ = decoder.push_bytes(b"\x1b[1mB\x1b[0m");
        let snap = decoder.snapshot();
        assert_eq!(snap.cells[0].ch, "B");
        assert_eq!(snap.cells[0].attrs & ATTR_BOLD, ATTR_BOLD);
    }

    #[test]
    fn snapshot_wide_char_spacer_is_empty() {
        // Two CJK chars take 4 grid columns: "新" + spacer + "版" + spacer.
        // The spacers must be empty so the renderer can rely on the wide
        // glyph itself painting ~2 cell widths in monospace fonts. If the
        // spacer leaks a placeholder character, the cursor (placed at
        // cursorCol * cellWidth) ends up inside already-typed text.
        let mut decoder = AnsiLineDecoder::new(20, 1);
        let _ = decoder.push_bytes("新版".as_bytes());
        let snap = decoder.snapshot();
        assert_eq!(snap.cells[0].ch, "新");
        assert_eq!(snap.cells[1].ch, "");
        assert_eq!(snap.cells[2].ch, "版");
        assert_eq!(snap.cells[3].ch, "");
        // Cursor advances 2 columns per wide char.
        assert_eq!(snap.cursor_col, 4);
        // Wide cells carry ATTR_WIDE so the frontend can pin their box
        // to exactly 2*cellWidth (avoids font-fallback width drift).
        assert_eq!(snap.cells[0].attrs & ATTR_WIDE, ATTR_WIDE);
        assert_eq!(snap.cells[2].attrs & ATTR_WIDE, ATTR_WIDE);
        // Spacer cells must NOT carry ATTR_WIDE — they contribute 0 width.
        assert_eq!(snap.cells[1].attrs & ATTR_WIDE, 0);
        assert_eq!(snap.cells[3].attrs & ATTR_WIDE, 0);
    }

    #[test]
    fn snapshot_respects_display_scrollback_offset() {
        let mut decoder = AnsiLineDecoder::new(12, 3);
        let _ = decoder.push_bytes(b"one\r\ntwo\r\nthree\r\nfour");

        let bottom = decoder.snapshot();
        assert_eq!(bottom.display_offset, 0);

        decoder.scroll_display(1);
        let scrolled = decoder.snapshot();
        assert_eq!(scrolled.display_offset, 1);
        assert_ne!(row_text(&bottom, 0), row_text(&scrolled, 0));
        assert!(!scrolled.cursor_visible);
    }

    fn row_text(snap: &GridSnapshot, row: usize) -> String {
        snap.cells[row * snap.cols as usize..(row + 1) * snap.cols as usize]
            .iter()
            .map(|cell| cell.ch.as_str())
            .collect::<String>()
    }
}
