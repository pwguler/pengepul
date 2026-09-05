//! The panel language every CLI verb renders with: a 64-column box, a
//! three-color palette, and the number formats. Knows nothing about what
//! is being shown — no Pools, Accounts, or admin payload here.

use std::ffi::OsStr;

/// Whether the views render panels or plain text. Decided once at the
/// edge from TTY and environment answers; the CLI core only sees the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Rich,
    Plain,
}

impl Style {
    /// `Rich` needs a TTY and no color-suppression variable; anything else —
    /// piped output, `NO_COLOR` set (even empty), `TERM=dumb` — is `Plain`.
    /// Plain is the only non-Rich mode: there is no uncolored panel.
    #[must_use]
    pub fn from_tty(is_tty: bool, no_color: Option<&OsStr>, term: Option<&OsStr>) -> Self {
        let no_color = no_color.is_some();
        let dumb = term == Some(OsStr::new("dumb"));
        if is_tty && !no_color && !dumb {
            Self::Rich
        } else {
            Self::Plain
        }
    }
}

/// One decimal at K/M scale, integers under 1,000. Integer math throughout,
/// so the rendered value is exact and truncation-free.
pub(crate) fn format_count(value: i64) -> String {
    let magnitude = value.unsigned_abs();
    let sign = if value < 0 { "-" } else { "" };
    if magnitude >= 1_000_000 {
        let whole = magnitude / 1_000_000;
        let tenths = (magnitude % 1_000_000) / 100_000;
        format!("{sign}{whole}.{tenths}M")
    } else if magnitude >= 1_000 {
        let whole = magnitude / 1_000;
        let tenths = (magnitude % 1_000) / 100;
        format!("{sign}{whole}.{tenths}K")
    } else {
        format!("{sign}{magnitude}")
    }
}

/// Comma-grouped form for request counts, which stay exact on the rollup line.
pub(crate) fn format_exact(value: i64) -> String {
    let magnitude = value.unsigned_abs().to_string();
    let sign = if value < 0 { "-" } else { "" };
    let mut grouped = String::with_capacity(magnitude.len() + magnitude.len() / 3);
    for (index, character) in magnitude.chars().enumerate() {
        if index > 0 && (magnitude.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    format!("{sign}{grouped}")
}

/// `1h2m` / `4m28s` / `59s` / `3d4h` for a duration: the same rounding
/// down the cooldown label uses, without borrowing its wording.
pub(crate) fn format_duration(seconds: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = seconds.max(0.0).floor() as u64;
    let days = total / 86_400;
    let hours = (total % 86_400) / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m{secs}s")
    } else {
        format!("{secs}s")
    }
}

pub(crate) const INNER_WIDTH: usize = PANEL_WIDTH - 4;

/// One `│ … │` row. The colored content is kept verbatim; only its *visible*
/// width is measured, and tail padding is added after it, so escape bytes
/// never fool the geometry. A row whose visible text overruns the fixed
/// width degrades to uncolored text clipped with an ellipsis — box
/// integrity wins over content, but the clip is always *marked*: a
/// silently cut path or url reads as a real one that does not exist.
pub(crate) fn panel_row(content: &str) -> String {
    let plain = strip_ansi(content);
    let visible = plain.chars().count();
    if visible > INNER_WIDTH {
        return format!("│ {} │", pad(&plain, INNER_WIDTH));
    }
    // A control character would split the row in two, neither half a box.
    // The sanitized text is the only safe thing to print, at the cost of
    // this row's color — the same trade the clip branch makes.
    let raw_control = content.chars().any(|c| c.is_control() && c != '\x1b');
    let tail = " ".repeat(INNER_WIDTH - visible);
    let body = if raw_control { &plain } else { content };
    format!("│ {body}{tail} │")
}

/// Strip ANSI escape sequences, keeping the visible text, and neutralise
/// control characters. A newline or tab is legal in an operator string (a
/// `--config` path, a `providers:` key) and would otherwise split a panel
/// row in two, neither half a box: `chars().count()` measures them as one
/// column while the terminal acts on them. Lives with the render code so
/// measurement and tests share one definition.
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\x1b' {
            while let Some(&next) = chars.peek() {
                chars.next();
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if character.is_control() {
            out.push(' ');
        } else {
            out.push(character);
        }
    }
    out
}

/// Top rule with a header inside: `┌─ <header> ─…┐`, filled to the same
/// width as a panel row. Box integrity wins over content, as in
/// `panel_row`: an oversized header (operator strings) is truncated to
/// the fixed width — and, as in `panel_row`, the truncation is *marked*.
/// An unmarked cut can land mid-number: `pool <49 chars> ─ 1` for a pool
/// holding 12 accounts is the same lie as a truncated token figure.
pub(crate) fn top_rule(header: &str) -> String {
    // Sanitized first: a control character in an operator string would
    // split the rule, and its width is measured here.
    let header = strip_ansi(header);
    let header: String = if header.chars().count() > INNER_WIDTH - 2 {
        pad(&header, INNER_WIDTH - 2)
    } else {
        header
    };
    let fill = INNER_WIDTH.saturating_sub(header.chars().count() + 1);
    let mut top = format!("┌─ {header} ");
    top.extend(std::iter::repeat_n('─', fill));
    top.push('┐');
    top
}

/// One labelled fact: the row shape every rich panel uses. The label is
/// the constant, the value the variable — a reader learns one grammar and
/// carries it across every verb (`consistent-panels`).
pub(crate) struct Fact {
    pub(crate) label: String,
    pub(crate) value: String,
}

impl Fact {
    pub(crate) fn new(label: &str, value: &str) -> Self {
        Self {
            label: label.to_string(),
            value: value.to_string(),
        }
    }
}

/// The widest a label column may grow before the value column suffers.
/// Labels are short nouns by construction — except a pool name, which is
/// an operator's config key and can be any length. Past this the label
/// clips; the value never does, because a truncated number is a lie.
const LABEL_WIDTH_CAP: usize = 14;

/// The label column for one panel: the longest label present plus two
/// columns of air, capped. Computed once per panel so values align down
/// the whole box rather than per section (AC-9).
pub(crate) fn label_column(facts: &[Fact]) -> usize {
    facts
        .iter()
        .map(|fact| strip_ansi(&fact.label).chars().count())
        .max()
        .unwrap_or(0)
        .min(LABEL_WIDTH_CAP)
        + 2
}

/// One fact as a panel row: the label clipped and padded to the panel's
/// column, then the value. Clipping the label is what keeps the value
/// column aligned and keeps an over-long label from pushing the value out
/// of the box, where `panel_row` would amputate it. Labels are plain text
/// by construction; the value carries the paint.
pub(crate) fn fact_row(fact: &Fact, column: usize) -> String {
    let label = pad(&strip_ansi(&fact.label), column.saturating_sub(2));
    format!("{label}  {}", fact.value)
}

/// A panel of labelled facts: header, aligned rows, bottom rule.
pub(crate) fn fact_panel(subject: &str, facts: &[Fact]) -> Vec<String> {
    let column = label_column(facts);
    let mut lines = vec![top_rule(subject)];
    for fact in facts {
        lines.push(panel_row(&fact_row(fact, column)));
    }
    lines.push(format!("└{}┘", "─".repeat(INNER_WIDTH + 2)));
    lines
}

/// A status glyph with its color: green success, amber attention, red failure.
pub(crate) fn status_glyph(kind: ActionGlyph) -> String {
    match kind {
        ActionGlyph::Ok => paint(GREEN, "●"),
        ActionGlyph::Attention => paint(AMBER, "●"),
        ActionGlyph::Failed => paint(RED, "●"),
    }
}

/// Which color an action status glyph carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActionGlyph {
    Ok,
    Attention,
    Failed,
}

/// The panel is a fixed 64 columns; every row is padded or truncated to it.
/// (60 could not hold the row columns without truncating most emails.)
pub(crate) const PANEL_WIDTH: usize = 64;

/// ANSI: green / amber / red / dim / bold — the whole palette.
pub(crate) const GREEN: &str = "\x1b[32m";

pub(crate) const AMBER: &str = "\x1b[33m";

pub(crate) const RED: &str = "\x1b[31m";

pub(crate) const DIM: &str = "\x1b[2m";

pub(crate) const BOLD: &str = "\x1b[1m";

pub(crate) const RESET: &str = "\x1b[0m";

/// Wrap one span in a color, always resetting after it, so color never
/// bleeds into neighbouring cells.
pub(crate) fn paint(color: &str, text: &str) -> String {
    format!("{color}{text}{RESET}")
}

/// Clamp to exactly `width` display columns: spaces pad short text and an
/// ellipsis replaces the last visible character of long text. A zero
/// width yields the empty string rather than underflowing — the guard
/// lives here, at the seam, not in every caller.
pub(crate) fn pad(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let characters: Vec<char> = text.chars().collect();
    if characters.len() > width {
        let mut clipped: String = characters[..width - 1].iter().collect();
        clipped.push('…');
        return clipped;
    }
    let mut padded = text.to_string();
    padded.extend(std::iter::repeat_n(' ', width - characters.len()));
    padded
}

/// A 10-cell share bar: `█` per whole tenth of the share, `░` for the rest,
/// with the integer percentage — `None` only when the total is 0.
pub(crate) fn share_bar(part: i64, total: i64) -> (String, Option<i64>) {
    if total == 0 {
        return ("░░░░░░░░░░".to_string(), None);
    }
    let share = (part.max(0).min(total) * 100).div_euclid(total);
    // share is clamped to 0..=100 above, so the division is exact and the
    // cast loses nothing.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = (share as usize).div_euclid(10);
    let mut bar = String::with_capacity(30);
    bar.extend(std::iter::repeat_n('█', filled));
    bar.extend(std::iter::repeat_n('░', 10 - filled));
    (bar, Some(share))
}

#[derive(Default)]
pub(crate) struct Output {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Output {
    pub(crate) fn line(&mut self, value: &str) {
        self.stdout.push_str(value);
        self.stdout.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::{Style, format_count, format_duration, format_exact, pad, paint, share_bar};
    use std::ffi::OsStr;

    #[test]
    fn style_is_rich_only_on_a_color_tty() {
        // Via the free function so the non-test build carries no dead shim.
        assert_eq!(Style::from_tty(true, None, None), Style::Rich);
        assert_eq!(Style::from_tty(false, None, None), Style::Plain);
        assert_eq!(
            Style::from_tty(true, Some(OsStr::new("1")), None),
            Style::Plain
        );
        assert_eq!(
            Style::from_tty(true, Some(OsStr::new("")), None),
            Style::Plain
        );
        assert_eq!(
            Style::from_tty(true, None, Some(OsStr::new("dumb"))),
            Style::Plain
        );
        assert_eq!(
            Style::from_tty(true, None, Some(OsStr::new("xterm-256color"))),
            Style::Rich
        );
    }

    #[test]
    fn duration_formats_by_magnitude() {
        assert_eq!(format_duration(59.0), "59s");
        assert_eq!(format_duration(268.0), "4m28s");
        assert_eq!(format_duration(3_661.0), "1h1m");
        assert_eq!(format_duration(104_400.0), "1d5h");
        assert_eq!(format_duration(2_592_000.0), "30d0h");
    }

    #[test]
    fn format_count_scales_tokens_with_one_decimal() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(7), "7");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.0K");
        assert_eq!(format_count(155_300), "155.3K");
        assert_eq!(format_count(812_300), "812.3K");
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(45_200_000), "45.2M");
        assert_eq!(format_count(-1), "-1");
    }

    #[test]
    fn format_exact_groups_request_counts_with_commas() {
        assert_eq!(format_exact(0), "0");
        assert_eq!(format_exact(6), "6");
        assert_eq!(format_exact(999), "999");
        assert_eq!(format_exact(1_000), "1,000");
        assert_eq!(format_exact(1_204), "1,204");
        assert_eq!(format_exact(999_999), "999,999");
        assert_eq!(format_exact(1_000_000), "1,000,000");
        assert_eq!(format_exact(-1), "-1");
    }

    #[test]
    fn pad_pads_to_the_fixed_width() {
        assert_eq!(pad("abc", 6), "abc   ");
        assert_eq!(pad("", 3), "   ");
        assert_eq!(pad("abcdef", 6), "abcdef");
    }

    #[test]
    fn pad_truncates_with_an_ellipsis_never_exceeding_width() {
        assert_eq!(pad("abcdefg", 5), "abcd…");
        assert_eq!(pad("abcdefg", 1), "…");
        assert_eq!(pad("abcdefg", 6).chars().count(), 6);
    }

    #[test]
    fn share_bar_fills_whole_tenths_of_the_share() {
        assert_eq!(share_bar(50, 100), ("█████░░░░░".to_string(), Some(50)));
        assert_eq!(share_bar(45, 100), ("████░░░░░░".to_string(), Some(45)));
        assert_eq!(share_bar(1, 3), ("███░░░░░░░".to_string(), Some(33)));
        assert_eq!(share_bar(7, 7), ("██████████".to_string(), Some(100)));
    }

    #[test]
    fn share_bar_without_a_pool_total_is_empty_cells() {
        assert_eq!(share_bar(0, 0), ("░░░░░░░░░░".to_string(), None));
    }

    #[test]
    fn paint_wraps_the_span_and_resets() {
        assert_eq!(paint("\x1b[33m", "x"), "\x1b[33mx\x1b[0m");
    }
}
