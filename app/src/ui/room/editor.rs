//! The message being typed: text with a cursor, wrapped into lines of the width it
//! is shown at, so long prompts can be read and edited whole.

/// Text and a cursor (a byte offset, always on a char boundary).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Editor {
    text: String,
    cursor: usize,
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The character just before the cursor.
    pub fn before_cursor(&self) -> Option<char> {
        self.text[..self.cursor].chars().next_back()
    }

    pub fn insert(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    pub fn backspace(&mut self) {
        if let Some(ch) = self.before_cursor() {
            self.cursor -= ch.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        if let Some(ch) = self.before_cursor() {
            self.cursor -= ch.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(ch) = self.text[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }

    /// Start of the cursor's line (as shown at `width`).
    pub fn home(&mut self, width: usize) {
        let (row, _) = self.position(width);
        self.cursor = self.lines(width)[row].0;
    }

    /// End of the cursor's line (as shown at `width`).
    pub fn end(&mut self, width: usize) {
        let (row, _) = self.position(width);
        self.cursor = self.lines(width)[row].1;
    }

    /// One line up or down (`delta` -1 or 1), keeping the column when it can.
    pub fn vertical(&mut self, delta: isize, width: usize) {
        let lines = self.lines(width);
        let (row, column) = self.position(width);
        let Some(target) = row.checked_add_signed(delta).filter(|r| *r < lines.len()) else {
            // Past the first or last line: to the start or the end.
            self.cursor = if delta < 0 { 0 } else { self.text.len() };
            return;
        };
        let (start, end) = lines[target];
        self.cursor = self.text[start..end]
            .char_indices()
            .nth(column)
            .map_or(end, |(offset, _)| start + offset);
    }

    /// Byte ranges of the lines the text shows at `width`: breaks at `\n` and wraps
    /// long lines at spaces when it can. Always at least one (empty) line.
    pub fn lines(&self, width: usize) -> Vec<(usize, usize)> {
        let width = width.max(1);
        let mut lines = Vec::new();
        let mut start = 0;
        for paragraph in self.text.split('\n') {
            let end_of_paragraph = start + paragraph.len();
            let mut line_start = start;
            let mut count = 0;
            let mut last_space = None;
            for (offset, ch) in paragraph.char_indices() {
                let at = start + offset;
                if count == width {
                    // Wrap after the last space on the line, or right here.
                    let cut = last_space.filter(|space| *space > line_start).unwrap_or(at);
                    lines.push((line_start, cut));
                    line_start = cut;
                    count = self.text[line_start..at].chars().count();
                    last_space = None;
                }
                if ch == ' ' {
                    last_space = Some(at + 1);
                }
                count += 1;
            }
            lines.push((line_start, end_of_paragraph));
            start = end_of_paragraph + 1;
        }
        lines
    }

    /// Row and column of the cursor at `width`.
    pub fn position(&self, width: usize) -> (usize, usize) {
        let lines = self.lines(width);
        // A cursor on a wrap point belongs to the next line, except at the very end.
        let row = lines
            .iter()
            .rposition(|(start, end)| *start <= self.cursor && self.cursor <= *end)
            .unwrap_or(0);
        let (start, _) = lines[row];
        (row, self.text[start..self.cursor].chars().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.insert_str(text);
        editor
    }

    fn shown(editor: &Editor, width: usize) -> Vec<&str> {
        editor
            .lines(width)
            .into_iter()
            .map(|(start, end)| &editor.text()[start..end])
            .collect()
    }

    #[test]
    fn wraps_at_spaces_and_breaks_at_newlines() {
        let editor = typed("write the tests\nfor the pix handler please");
        assert_eq!(
            shown(&editor, 12),
            ["write the ", "tests", "for the pix ", "handler ", "please"]
        );
        assert_eq!(shown(&typed("abcdefghij"), 4), ["abcd", "efgh", "ij"]);
        assert_eq!(shown(&Editor::default(), 10), [""]);
    }

    #[test]
    fn edits_in_the_middle() {
        let mut editor = typed("hello world");
        for _ in 0..5 {
            editor.left();
        }
        editor.backspace();
        editor.insert(',');
        editor.insert(' ');
        assert_eq!(editor.text(), "hello, world");
        editor.home(80);
        editor.delete();
        assert_eq!(editor.text(), "ello, world");
        editor.end(80);
        editor.insert('!');
        assert_eq!(editor.text(), "ello, world!");
    }

    #[test]
    fn moves_between_lines_keeping_the_column() {
        let mut editor = typed("first line\nsecond\nthird line");
        assert_eq!(editor.position(80), (2, 10));
        editor.vertical(-1, 80);
        assert_eq!(editor.position(80), (1, 6));
        editor.vertical(-1, 80);
        assert_eq!(editor.position(80), (0, 6));
        editor.vertical(-1, 80);
        assert_eq!(editor.position(80), (0, 0));
        editor.insert('>');
        assert_eq!(editor.text(), ">first line\nsecond\nthird line");
    }

    #[test]
    fn handles_multibyte_text() {
        let mut editor = typed("ação é");
        editor.left();
        editor.backspace();
        assert_eq!(editor.text(), "açãoé");
        assert_eq!(editor.before_cursor(), Some('o'));
    }
}
