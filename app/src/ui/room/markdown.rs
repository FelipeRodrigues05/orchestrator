//! An agent's conversation as readable text: the Markdown agents answer in (titles,
//! lists, **bold**, `code`, fenced code blocks, tables) styled for the grid, tool
//! lines dimmed, the operator's messages marked. Used by the console and the
//! reader view (F6).

use orch_core::model::{LogEntry, LogLevel};

use super::{ACCENT, DIM, FAINT, FRAME, TEXT, TITLE, WARN, s};
use crate::ui::grid::Style;

/// One shown row, already styled.
pub(super) type Row = Vec<(String, Style)>;

/// Rows of `entries` (one agent's log lines, oldest first) at `width` columns.
pub(super) fn render(entries: &[&LogEntry], width: usize) -> Vec<Row> {
    let width = width.max(16);
    let mut rows: Vec<Row> = Vec::new();
    // Inside a fenced code block: its rows are boxed and never restyled.
    let mut code: Option<String> = None;
    for entry in entries {
        if entry.level == LogLevel::Operator {
            code = None;
            push_gap(&mut rows);
            let text = styled(&entry.text, TEXT.bold());
            let mut lines = wrap(prefixed(&s("› ", ACCENT.bold()), text), width, 2);
            rows.append(&mut lines);
            push_gap(&mut rows);
            continue;
        }
        let text = entry.text.as_str();
        let trimmed = text.trim_start();
        if let Some(fence) = trimmed.strip_prefix("```") {
            if code.take().is_some() {
                rows.push(vec![s(format!("└{}", "─".repeat(width - 1)), FRAME)]);
            } else {
                let language = fence.trim();
                let label = if language.is_empty() {
                    String::new()
                } else {
                    format!(" {language} ")
                };
                let rest = width.saturating_sub(label.chars().count() + 2);
                rows.push(vec![
                    s("┌─", FRAME),
                    s(label, DIM),
                    s("─".repeat(rest), FRAME),
                ]);
                code = Some(language.to_owned());
            }
            continue;
        }
        if code.is_some() {
            let chars: Vec<char> = text.chars().collect();
            let room = width.saturating_sub(2).max(1);
            if chars.is_empty() {
                rows.push(vec![s("│", FRAME)]);
            }
            for chunk in chars.chunks(room) {
                rows.push(vec![
                    s("│ ", FRAME),
                    s(chunk.iter().collect::<String>(), ACCENT),
                ]);
            }
            continue;
        }
        rows.append(&mut line(entry, text, width));
    }
    if code.is_some() {
        // An answer still streaming: close the box so it reads as one.
        rows.push(vec![s(format!("└{}", "─".repeat(width - 1)), FRAME)]);
    }
    rows
}

/// One line outside code blocks.
fn line(entry: &LogEntry, text: &str, width: usize) -> Vec<Row> {
    let trimmed = text.trim_start();
    let indent = text.len() - trimmed.len();
    // Tool calls and turn ends, as the runtime logs them.
    if trimmed.starts_with('▸') || trimmed.starts_with('■') {
        return wrap(styled(trimmed, FAINT), width, 2);
    }
    if trimmed.starts_with('✖') || matches!(entry.level, LogLevel::Warn | LogLevel::Error) {
        return wrap(styled(trimmed, WARN), width, 2);
    }
    if trimmed.is_empty() {
        return vec![Vec::new()];
    }
    if let Some(title) = heading(trimmed) {
        return wrap(styled(title, TITLE), width, 0);
    }
    if trimmed.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')) && trimmed.len() >= 3 {
        return vec![vec![s("─".repeat(width), FAINT)]];
    }
    if trimmed.starts_with('|') {
        return table_row(trimmed, width);
    }
    if let Some(quote) = trimmed.strip_prefix('>') {
        return wrap(
            prefixed(&s("▌ ", DIM), styled(quote.trim_start(), DIM)),
            width,
            2,
        );
    }
    let pad = " ".repeat(indent.min(8));
    for marker in ["- ", "* ", "+ "] {
        if let Some(item) = trimmed.strip_prefix(marker) {
            let bullet = s(format!("{pad}• "), ACCENT);
            return wrap(
                prefixed(&bullet, styled(item, TEXT)),
                width,
                indent.min(8) + 2,
            );
        }
    }
    if let Some((number, item)) = trimmed.split_once(". ")
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
    {
        let label = format!("{pad}{number}. ");
        let hang = label.chars().count();
        return wrap(prefixed(&s(label, ACCENT), styled(item, TEXT)), width, hang);
    }
    wrap(
        prefixed(&s(pad, TEXT), styled(trimmed, TEXT)),
        width,
        indent.min(8),
    )
}

/// `## Title` → `Title`.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' '))
        .flatten()
        .map(str::trim)
}

/// `| a | b |` with drawn rails; `|---|---|` becomes a rule.
fn table_row(line: &str, width: usize) -> Vec<Row> {
    if line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
        let rule = line
            .trim()
            .chars()
            .map(|c| if c == '|' { '┼' } else { '─' })
            .collect::<String>();
        return vec![vec![s(rule.chars().take(width).collect::<String>(), FRAME)]];
    }
    let mut chars = Vec::new();
    for (index, cell) in line.split('|').enumerate() {
        if index > 0 {
            chars.push(('│', FRAME));
        }
        chars.extend(styled(cell, TEXT));
    }
    wrap(chars, width, 2)
}

/// A blank row between messages, never two.
fn push_gap(rows: &mut Vec<Row>) {
    if rows.last().is_some_and(|row| !row.is_empty()) {
        rows.push(Vec::new());
    }
}

fn prefixed(prefix: &(String, Style), mut rest: Vec<(char, Style)>) -> Vec<(char, Style)> {
    let mut chars: Vec<(char, Style)> = prefix.0.chars().map(|c| (c, prefix.1)).collect();
    chars.append(&mut rest);
    chars
}

/// Inline Markdown: `**bold**` and `` `code` `` change the style; the markers go.
fn styled(text: &str, base: Style) -> Vec<(char, Style)> {
    let mut out = Vec::new();
    let (mut bold, mut code) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => code = !code,
            '*' if !code && chars.peek() == Some(&'*') => {
                chars.next();
                bold = !bold;
            }
            c => {
                let style = match (code, bold) {
                    (true, _) => ACCENT,
                    (false, true) => base.bold(),
                    (false, false) => base,
                };
                out.push((c, style));
            }
        }
    }
    out
}

/// Styled characters in rows of `width`, broken at spaces when it can; rows after
/// the first start `hang` columns in.
fn wrap(chars: Vec<(char, Style)>, width: usize, hang: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut rest = chars;
    let mut room = width;
    loop {
        if rest.len() <= room {
            rows.push(spans(&rest));
            break;
        }
        let cut = rest[..=room]
            .iter()
            .rposition(|(c, _)| *c == ' ')
            .filter(|i| *i > room / 3)
            .unwrap_or(room);
        rows.push(spans(&rest[..cut]));
        rest.drain(..cut);
        while rest.first().is_some_and(|(c, _)| *c == ' ') {
            rest.remove(0);
        }
        let hang = hang.min(width / 2);
        rest.splice(0..0, std::iter::repeat_n((' ', TEXT), hang));
        room = width;
    }
    rows
}

/// Consecutive characters of one style as one span.
fn spans(chars: &[(char, Style)]) -> Row {
    let mut row: Row = Vec::new();
    for &(c, style) in chars {
        match row.last_mut() {
            Some((text, last)) if *last == style => text.push(c),
            _ => row.push((c.to_string(), style)),
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> LogEntry {
        LogEntry {
            at: chrono::Utc::now(),
            agent_id: None,
            level: LogLevel::Info,
            text: text.into(),
        }
    }

    fn plain(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| row.iter().map(|(t, _)| t.as_str()).collect())
            .collect()
    }

    #[test]
    fn markdown_reads_as_text() {
        let lines = [
            "#### L2 — `sidebarOpen` cookie",
            "**Fix:** add `lowercase` to the rules",
            "- one",
            "```php",
            "Password::defaults(fn () => null);",
            "```",
            "| Area | Status |",
            "|------|--------|",
            "| Auth | Covered |",
            "▸ Write tests/Unit/ExtractedJobTest.php",
        ];
        let entries: Vec<LogEntry> = lines.iter().map(|l| entry(l)).collect();
        let refs: Vec<&LogEntry> = entries.iter().collect();
        let rows = render(&refs, 40);
        let text = plain(&rows);
        assert_eq!(text[0], "L2 — sidebarOpen cookie");
        assert_eq!(text[1], "Fix: add lowercase to the rules");
        assert_eq!(text[2], "• one");
        assert!(text[3].starts_with("┌─ php ─"));
        assert_eq!(text[4], "│ Password::defaults(fn () => null);");
        assert!(text[5].starts_with("└─"));
        assert_eq!(text[6], "│ Area │ Status │");
        assert!(text[7].starts_with("┼──"));
        // Styles: the title, bold label and inline code stand out; tools fade.
        assert_eq!(rows[0][0].1, TITLE);
        assert_eq!(rows[1][0].1, TEXT.bold());
        assert!(
            rows[1]
                .iter()
                .any(|(t, style)| t == "lowercase" && *style == ACCENT)
        );
        assert_eq!(rows.last().unwrap()[0].1, FAINT);
    }

    #[test]
    fn long_lines_wrap_under_their_bullet() {
        let entries = [entry("- a long item that has to wrap onto the next row")];
        let refs: Vec<&LogEntry> = entries.iter().collect();
        let text = plain(&render(&refs, 20));
        assert_eq!(text[0], "• a long item that");
        assert!(text[1].starts_with("  "), "{text:?}");
    }
}
