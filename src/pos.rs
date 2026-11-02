//! Compiler locations (1-based lines, 1-based columns in code points) to and
//! from LSP positions (0-based lines, 0-based UTF-16 columns).

use lsp_types::{Position, Range};
use ares_compiler::ast::Loc;

/// Text that can hand out its lines (0-based, as `str::lines` splits them).
pub trait Lines {
    fn line(&self, n: usize) -> Option<&str>;
}

impl Lines for str {
    fn line(&self, n: usize) -> Option<&str> {
        self.lines().nth(n)
    }
}

/// A text with its line starts, so finding a line needs no scan from the top.
#[derive(Clone, Default)]
pub struct Text {
    text: String,
    starts: Vec<usize>,
}

impl Text {
    pub fn new(text: String) -> Self {
        let starts = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
        Text { text, starts }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The byte offset of LSP position `p`, clamped to its line and the text.
    pub fn offset(&self, p: Position) -> usize {
        let Some(&start) = self.starts.get(p.line as usize) else { return self.text.len() };
        let line = self.line(p.line as usize).unwrap_or("");
        let mut units = 0;
        for (i, c) in line.char_indices() {
            if units >= p.character {
                return start + i;
            }
            units += c.len_utf16() as u32;
        }
        start + line.len()
    }

    /// Replaces `range` with `new` (an incremental `didChange`).
    pub fn edit(&mut self, range: Range, new: &str) {
        let (start, end) = (self.offset(range.start), self.offset(range.end));
        let mut text = std::mem::take(&mut self.text);
        text.replace_range(start..end.max(start), new);
        *self = Text::new(text);
    }
}

impl Lines for Text {
    fn line(&self, n: usize) -> Option<&str> {
        let start = *self.starts.get(n)?;
        if start == self.text.len() {
            return None;
        }
        let end = self.starts.get(n + 1).map_or(self.text.len(), |e| e - 1);
        let l = &self.text[start..end];
        Some(l.strip_suffix('\r').unwrap_or(l))
    }
}

/// The UTF-16 offset of 1-based code-point column `col` in `line`.
fn utf16_col(line: &str, col: i32) -> u32 {
    line.chars().take((col - 1).max(0) as usize).map(|c| c.len_utf16() as u32).sum()
}

/// The 1-based code-point column of UTF-16 offset `character` in `line`.
pub fn code_point_col(line: &str, character: u32) -> i32 {
    let mut units = 0;
    let mut col = 1;
    for c in line.chars() {
        if units >= character {
            break;
        }
        units += c.len_utf16() as u32;
        col += 1;
    }
    col
}

pub fn position<L: Lines + ?Sized>(text: &L, line: i32, col: i32) -> Position {
    let l = (line - 1).max(0) as u32;
    let line_text = text.line(l as usize).unwrap_or("");
    Position { line: l, character: utf16_col(line_text, col) }
}

pub fn range<L: Lines + ?Sized>(text: &L, loc: &Loc) -> Range {
    Range { start: position(text, loc.line, loc.column), end: position(text, loc.end_line, loc.end_column) }
}

/// `(line, col)` in compiler terms for an LSP position.
pub fn from_lsp<L: Lines + ?Sized>(text: &L, p: Position) -> (i32, i32) {
    let line_text = text.line(p.line as usize).unwrap_or("");
    (p.line as i32 + 1, code_point_col(line_text, p.character))
}

/// Whether compiler position `(line, col)` lies in `loc` (end inclusive, so a
/// cursor just after a word still hits it).
pub fn contains(loc: &Loc, line: i32, col: i32) -> bool {
    (loc.line, loc.column) <= (line, col) && (line, col) <= (loc.end_line, loc.end_column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_utf16() {
        let text = "a😀b\nxy";
        assert_eq!(position(text, 1, 3), Position { line: 0, character: 3 }, "the emoji is two UTF-16 units");
        assert_eq!(from_lsp(text, Position { line: 0, character: 3 }), (1, 3));
        assert_eq!(from_lsp(text, Position { line: 1, character: 1 }), (2, 2));
    }

    #[test]
    fn incremental_edits() {
        let p = |line, character| Position { line, character };
        let mut t = Text::new("a😀b\r\nxy\n".to_owned());
        t.edit(Range { start: p(0, 3), end: p(1, 1) }, "Z"); // after the emoji's two units, up to `y`
        assert_eq!(t.as_str(), "a😀Zy\n");
        t.edit(Range { start: p(1, 0), end: p(1, 0) }, "new"); // at the empty last line
        assert_eq!(t.as_str(), "a😀Zy\nnew");
        t.edit(Range { start: p(0, 0), end: p(9, 0) }, ""); // past the end clamps
        assert_eq!(t.as_str(), "");
        assert_eq!(t.line(0), None);
    }

    #[test]
    fn text_lines_match_str_lines() {
        for s in ["", "a", "a\n", "a\nb", "a\r\nb\r\n", "\n\nx", "a\n\n"] {
            let t = Text::new(s.to_owned());
            let got: Vec<_> = (0..5).map_while(|n| t.line(n)).collect();
            assert_eq!(got, s.lines().collect::<Vec<_>>(), "{s:?}");
        }
    }
}
