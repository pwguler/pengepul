//! The model picker: the list `pengepul launch` offers when no model was
//! named, and the keys that move through it.
//!
//! Not a Panel — a Panel carries facts about one subject, and this is a
//! menu the operator answers (CONTEXT.md, Panel). It keeps the palette and
//! takes the same `Style` every other surface takes, while staying outside
//! the box. Split out of `runtime.rs`, which is the adapter that makes a
//! verb touch the real world: here the frame is a value a test can assert
//! rather than something only a pty could observe. `draw_picker` writes to
//! any `Write` and `picker_loop` reads from any event source.

use anyhow::{Context as _, Result};

use crate::cli::{ModelChoice, matching_choices};
use crate::render::{BOLD, DIM, GREEN, Style, format_count, pad, paint};

/// Drive the model picker on the terminal: the module's whole interface.
/// Everything below it — the frame, the key loop, the guard — is
/// implementation.
///
/// # Errors
///
/// Returns an error if the terminal cannot be driven.
///
/// Drive the model picker on the terminal: arrows move, typing narrows,
/// enter takes the highlighted row.
///
/// It runs on the alternate screen so the operator's scrollback survives,
/// and raw mode is left again on every exit — the error path included,
/// which is why the loop's outcome is captured before the terminal is
/// restored rather than returned through `?`.
pub fn pick_model(harness: &str, choices: &[ModelChoice], style: Style) -> Result<Option<String>> {
    use crossterm::{cursor, event, execute, terminal};

    terminal::enable_raw_mode()
        .context("failed to put the terminal in raw mode; pass --model to skip the picker")?;
    let mut screen = std::io::stderr();
    // From here the terminal is only put back by the guard, so an unwind
    // through the loop puts it back too.
    let restore = TerminalGuard;
    let entered = execute!(screen, terminal::EnterAlternateScreen, cursor::Hide);
    let outcome = entered.map_err(anyhow::Error::from).and_then(|()| {
        picker_loop(&mut screen, harness, choices, style, &mut || {
            event::read().map_err(anyhow::Error::from)
        })
    });
    drop(restore);
    outcome
}

/// Puts the terminal back however the picker leaves.
///
/// Raw mode under the alternate screen is not a state to hand an operator:
/// no echo, no line editing, Ctrl-C dead, cursor invisible, and `reset`
/// typed blind the only way out. A guard holds for the error paths and for
/// a panic unwinding through the loop; two statements after the call do
/// not.
///
/// A signal that terminates the process runs no destructor, so `kill` and
/// `timeout` still leave the terminal raw. That gap is real, and named in
/// the spec rather than papered over here.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut screen = std::io::stderr();
        let _ = crossterm::execute!(
            screen,
            crossterm::cursor::Show,
            crossterm::terminal::LeaveAlternateScreen
        );
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// The picker's state machine, over whatever `next_event` yields. Split
/// from the terminal setup so the loop is the part with the logic and the
/// setup is the part with the cleanup.
fn picker_loop(
    screen: &mut impl std::io::Write,
    harness: &str,
    choices: &[ModelChoice],
    style: Style,
    next_event: &mut dyn FnMut() -> Result<crossterm::event::Event>,
) -> Result<Option<String>> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    let mut filter = String::new();
    let mut cursor = 0usize;
    let mut scroll = 0usize;
    loop {
        let matching = matching_choices(choices, &filter);
        cursor = cursor.min(matching.len().saturating_sub(1));
        let rows = draw_picker(
            screen,
            &PickerFrame {
                harness,
                total: choices.len(),
                matching: &matching,
                filter: &filter,
                cursor,
                style,
                size: crossterm::terminal::size().unwrap_or((80, 24)),
            },
            &mut scroll,
        )?;
        let Event::Key(key) = next_event()? else {
            continue;
        };
        // A key repeats as Press and Release on terminals that report both;
        // acting on one of them keeps a single press from moving twice.
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let page = rows.max(1);
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            KeyCode::Up => cursor = cursor.saturating_sub(1),
            KeyCode::Down => cursor = (cursor + 1).min(matching.len().saturating_sub(1)),
            KeyCode::PageUp => cursor = cursor.saturating_sub(page),
            KeyCode::PageDown => cursor = (cursor + page).min(matching.len().saturating_sub(1)),
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = matching.len().saturating_sub(1),
            KeyCode::Backspace => {
                filter.pop();
                cursor = 0;
                scroll = 0;
            }
            // Ctrl-U clears the line everywhere else; crossterm reports it
            // as Char('u') with CONTROL, so untreated it typed a `u` and
            // narrowed the list instead of widening it.
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                filter.clear();
                cursor = 0;
                scroll = 0;
            }
            // `stty erase ^H` and PuTTY send Ctrl-H for Backspace.
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                filter.pop();
                cursor = 0;
                scroll = 0;
            }
            KeyCode::Enter => {
                if let Some(choice) = matching.get(cursor) {
                    return Ok(Some(choice.id.clone()));
                }
            }
            // Any other chord is a command this picker does not have, not
            // text. Typing its bare letter into the search was never what
            // the operator meant by Ctrl-W or Alt-B.
            KeyCode::Char(_)
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {}
            KeyCode::Char(character) => {
                filter.push(character);
                // A narrower list means the old row number means nothing.
                cursor = 0;
                scroll = 0;
            }
            _ => {}
        }
    }
}

/// What one frame of the picker is drawn from. `scroll` stays out of it:
/// the frame decides it from the cursor and hands it back to the loop.
struct PickerFrame<'a> {
    harness: &'a str,
    total: usize,
    matching: &'a [ModelChoice],
    filter: &'a str,
    cursor: usize,
    style: Style,
    /// Columns and rows. Handed in rather than asked of the terminal, so a
    /// test can put the frame in a 6-row window and read what comes out —
    /// which is where the overflows lived.
    size: (u16, u16),
}

/// What is being launched, and how much of the catalog the search left.
///
/// Built plain, fitted, and only then painted: a colour code has no width,
/// and measuring after painting is how a line ends up wider than the
/// terminal.
fn heading_line(
    harness: &str,
    shown: usize,
    total: usize,
    width: usize,
    ink: &impl Fn(&str, &str) -> String,
) -> String {
    let counter = format!("{shown}/{total}");
    let subject = format!("launch {harness}");
    let room = width.saturating_sub(4);
    let gap = room
        .saturating_sub(subject.chars().count() + counter.chars().count())
        .max(1);
    let subject_room = room.saturating_sub(counter.chars().count() + gap);
    format!(
        "  {}{}{}",
        ink(BOLD, &pad(&subject, subject_room)),
        " ".repeat(gap),
        ink(DIM, &counter),
    )
}

/// What has been typed, behind a prompt glyph and a block cursor: the one
/// place typing goes, said without the word "search" in front of it. A
/// long filter keeps its tail, which is the part being typed.
fn query_line(filter: &str, width: usize, ink: &impl Fn(&str, &str) -> String) -> String {
    let room = width.saturating_sub(6);
    let typed = filter.chars().count();
    let shown: String = if typed > room {
        filter.chars().skip(typed - room).collect()
    } else {
        filter.to_string()
    };
    format!("  {} {shown}{}", ink(GREEN, "›"), ink(BOLD, "█"))
}

/// Paint one frame and return how many model rows fit, which is also the
/// page size the loop pages by.
///
/// Three zones with air between them: what you are doing and how much of
/// the catalog is left, what you have typed, and the list. Everything but
/// the list recedes, because the list is what is being read.
fn draw_picker(
    screen: &mut impl std::io::Write,
    frame_state: &PickerFrame<'_>,
    scroll: &mut usize,
) -> Result<usize> {
    use crossterm::{cursor as term_cursor, execute, terminal};

    let &PickerFrame {
        harness,
        total,
        matching,
        filter,
        cursor,
        style,
        size,
    } = frame_state;

    let (columns, lines) = size;
    let width = usize::from(columns).max(20);
    // Heading, blank, query, blank, and the footer: five rows that are not
    // the list.
    let rows = usize::from(lines).saturating_sub(5).max(1);
    if cursor < *scroll {
        *scroll = cursor;
    } else if cursor >= *scroll + rows {
        *scroll = cursor + 1 - rows;
    }
    *scroll = (*scroll).min(matching.len().saturating_sub(rows.min(matching.len())));

    let id_width = matching
        .iter()
        .map(|choice| choice.id.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(12, width.saturating_sub(34).max(12));
    // Rendered here, where the column widths are known, rather than carried
    // in as strings the picker could not re-fit.
    let contexts: Vec<String> = matching.iter().map(context_column).collect();
    let prices: Vec<String> = matching.iter().map(price_column).collect();
    let context_width = contexts
        .iter()
        .map(|text| text.chars().count())
        .max()
        .unwrap_or(0);

    // `paint` writes escapes unconditionally, so the Style decision made
    // once at the edge is applied here rather than ignored: NO_COLOR and
    // TERM=dumb reach this surface like every other one.
    let ink = |colour: &str, text: &str| match style {
        Style::Plain => text.to_string(),
        Style::Rich => paint(colour, text),
    };

    let mut frame = vec![
        heading_line(harness, matching.len(), total, width, &ink),
        String::new(),
        query_line(filter, width, &ink),
        String::new(),
    ];

    // One line of its own, counted against the same budget as a row, so
    // the frame stays exactly as tall as the terminal and the heading with
    // its counter is not scrolled away at the moment it is needed.
    let mut painted = 0;
    if matching.is_empty() {
        frame.push(ink(DIM, "    nothing matches"));
        painted += 1;
    }
    for (offset, choice) in matching.iter().skip(*scroll).take(rows).enumerate() {
        let selected = *scroll + offset == cursor;
        // Fitted while it is still plain, then painted. A row assembled from
        // three already-coloured columns cannot be measured, and a terminal
        // narrower than the columns want wrapped every row of the list.
        let id = pad(&choice.id, id_width);
        let context = format!("{:>context_width$}", contexts[*scroll + offset]);
        let price = &prices[*scroll + offset];
        let tail = pad(
            &format!("{context}  {price}"),
            width.saturating_sub(2 + id.chars().count() + 2),
        );
        let (context, price) = tail.split_at(
            tail.char_indices()
                .nth(context.chars().count())
                .map_or(tail.len(), |(index, _)| index),
        );
        frame.push(format!(
            "{} {}  {}{}",
            if selected {
                ink(GREEN, "❯")
            } else {
                " ".to_string()
            },
            // The pool prefix repeats down the whole list, so it is dimmed
            // and the model name keeps the reader's attention.
            paint_id(&id, selected, style),
            ink(DIM, context),
            ink(DIM, price),
        ));
        painted += 1;
    }
    for _ in painted..rows {
        frame.push(String::new());
    }
    // Fitted like everything else: on a narrow terminal the hint is the
    // line that can lose its tail without costing the operator a fact.
    frame.push(ink(DIM, &pad("  ↑↓ move   ⏎ run   esc cancel", width)));

    // Never taller than the terminal: the heading carries the counter, and
    // scrolling it away costs the operator the one number that says how
    // much the search cut.
    let height = usize::from(lines).max(1);
    if frame.len() > height {
        frame.truncate(height);
    }

    execute!(
        screen,
        term_cursor::MoveTo(0, 0),
        terminal::Clear(terminal::ClearType::All)
    )?;
    screen.write_all(frame.join("\r\n").as_bytes())?;
    screen.flush()?;
    Ok(rows)
}

/// A model's context window as the list shows it, or nothing where the
/// catalog did not say. A blank column is honest where a zero would not be.
fn context_column(choice: &ModelChoice) -> String {
    choice.context_window.map_or_else(String::new, |window| {
        format!(
            "{} ctx",
            format_count(i64::try_from(window).unwrap_or(i64::MAX))
        )
    })
}

/// What a million tokens costs, in and out.
fn price_column(choice: &ModelChoice) -> String {
    choice.price.map_or_else(String::new, |price| {
        format!("${:.2}/${:.2}", price.input, price.output)
    })
}

/// One model id, already padded: the `<pool>/` prefix dim and the model
/// name bright, so a column of `commandcode/...` reads as its models
/// rather than as its pool. The highlighted row is bright throughout.
fn paint_id(padded: &str, selected: bool, style: Style) -> String {
    if style == Style::Plain {
        return padded.to_string();
    }
    if selected {
        return paint(BOLD, padded);
    }
    let Some(slash) = padded.find('/') else {
        return padded.to_string();
    };
    let (prefix, rest) = padded.split_at(slash + 1);
    format!("{}{rest}", paint(DIM, prefix))
}

#[cfg(test)]
mod tests {
    use super::{PickerFrame, draw_picker, picker_loop};
    use crate::cli::ModelChoice;
    use crate::render::Style;
    use crate::render::{BOLD, DIM, GREEN};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    fn choices() -> Vec<ModelChoice> {
        [
            "anthropic/claude-opus-5",
            "anthropic/claude-haiku-4-5",
            "groq/llama",
        ]
        .iter()
        .map(|id| ModelChoice {
            id: (*id).to_string(),
            context_window: Some(1_000_000),
            price: None,
        })
        .collect()
    }

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Drive the picker's key loop over a scripted sequence, discarding
    /// what it paints. Running out of keys is an error rather than a
    /// silent cancel, so a test cannot pass by exhausting the script.
    fn drive(keys: Vec<Event>) -> Option<String> {
        let mut screen = Vec::new();
        let mut queued = keys.into_iter();
        picker_loop(&mut screen, "claude", &choices(), Style::Plain, &mut || {
            queued
                .next()
                .ok_or_else(|| anyhow::anyhow!("ran out of keys"))
        })
        .expect("the picker loop")
    }

    /// The frame `draw_picker` paints, as the lines a terminal would show.
    /// Finding the overflows below needed a pty before the frame was a
    /// value; now it is a string.
    fn frame(choices: &[ModelChoice], filter: &str, size: (u16, u16)) -> Vec<String> {
        let mut screen = Vec::new();
        let mut scroll = 0;
        draw_picker(
            &mut screen,
            &PickerFrame {
                harness: "claude",
                total: choices.len(),
                matching: choices,
                filter,
                cursor: 0,
                style: Style::Plain,
                size,
            },
            &mut scroll,
        )
        .expect("a frame");
        let painted = String::from_utf8(screen).expect("utf-8");
        // The frame is preceded by a cursor move and a clear, which position
        // it rather than being part of it.
        let body = painted
            .rsplit_once("\x1b[2J")
            .map_or(painted.as_str(), |(_, rest)| rest);
        body.split("\r\n").map(ToString::to_string).collect()
    }

    #[test]
    fn the_frame_is_never_taller_than_the_terminal() {
        // The heading carries the counter, and scrolling it away costs the
        // operator the one number that says how much the search cut.
        for rows in 1..=24u16 {
            let lines = frame(&choices(), "", (80, rows));
            assert!(
                lines.len() <= usize::from(rows),
                "{rows}-row terminal got {} lines",
                lines.len()
            );
        }
    }

    #[test]
    fn no_frame_line_is_wider_than_the_terminal() {
        for columns in [20u16, 30, 40, 80, 200] {
            for line in frame(&choices(), "", (columns, 24)) {
                assert!(
                    line.chars().count() <= usize::from(columns),
                    "{columns}-column terminal got a {}-char line: {line:?}",
                    line.chars().count()
                );
            }
        }
    }

    #[test]
    fn a_long_query_does_not_push_the_frame_over() {
        // The query line was the one line with no width budget, so a pasted
        // model id re-scrolled the heading on every keystroke.
        let typed = "x".repeat(200);
        let lines = frame(&choices(), &typed, (80, 24));
        assert!(lines.len() <= 24);
        for line in &lines {
            assert!(line.chars().count() <= 80, "{line:?}");
        }
        // What is kept is the end, which is the part being typed.
        assert!(lines[2].contains('x'));
    }

    #[test]
    fn the_heading_counts_what_the_search_left() {
        let all = choices();
        let matching: Vec<ModelChoice> = all
            .iter()
            .filter(|choice| choice.id.contains("haiku"))
            .cloned()
            .collect();
        let mut screen = Vec::new();
        let mut scroll = 0;
        draw_picker(
            &mut screen,
            &PickerFrame {
                harness: "claude",
                total: all.len(),
                matching: &matching,
                filter: "haiku",
                cursor: 0,
                style: Style::Plain,
                size: (80, 24),
            },
            &mut scroll,
        )
        .expect("a frame");
        let painted = String::from_utf8(screen).expect("utf-8");
        assert!(painted.contains("1/3"), "{painted}");
        assert!(painted.contains("launch claude"), "{painted}");
    }

    #[test]
    fn plain_style_paints_no_colour() {
        // NO_COLOR and TERM=dumb reach this surface like every other one.
        let painted = frame(&choices(), "", (80, 24)).join("");
        for colour in [BOLD, DIM, GREEN, "\x1b[0m"] {
            assert!(!painted.contains(colour), "{painted:?}");
        }
    }

    #[test]
    fn an_empty_list_says_so_and_still_fits() {
        let lines = frame(&[], "zzz", (80, 8));
        assert!(lines.iter().any(|line| line.contains("nothing matches")));
        assert!(lines.len() <= 8);
    }

    #[test]
    fn enter_takes_the_highlighted_row() {
        assert_eq!(
            drive(vec![press(KeyCode::Enter)]).as_deref(),
            Some("anthropic/claude-opus-5")
        );
    }

    #[test]
    fn the_arrows_move_the_highlight() {
        assert_eq!(
            drive(vec![press(KeyCode::Down), press(KeyCode::Enter)]).as_deref(),
            Some("anthropic/claude-haiku-4-5")
        );
        // Up at the top stays at the top rather than wrapping or panicking.
        assert_eq!(
            drive(vec![press(KeyCode::Up), press(KeyCode::Enter)]).as_deref(),
            Some("anthropic/claude-opus-5")
        );
        assert_eq!(
            drive(vec![press(KeyCode::End), press(KeyCode::Enter)]).as_deref(),
            Some("groq/llama")
        );
    }

    #[test]
    fn typing_narrows_and_backspace_widens() {
        assert_eq!(
            drive(vec![
                press(KeyCode::Char('h')),
                press(KeyCode::Char('a')),
                press(KeyCode::Enter)
            ])
            .as_deref(),
            Some("anthropic/claude-haiku-4-5")
        );
        // `ha` then a backspace leaves `h`, which still excludes opus.
        assert_eq!(
            drive(vec![
                press(KeyCode::Char('z')),
                press(KeyCode::Backspace),
                press(KeyCode::Enter)
            ])
            .as_deref(),
            Some("anthropic/claude-opus-5")
        );
    }

    #[test]
    fn esc_and_ctrl_c_cancel() {
        assert_eq!(drive(vec![press(KeyCode::Esc)]), None);
        assert_eq!(
            drive(vec![Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL
            ))]),
            None
        );
    }

    #[test]
    fn enter_on_no_match_neither_picks_nor_panics() {
        assert_eq!(
            drive(vec![
                press(KeyCode::Char('z')),
                press(KeyCode::Enter),
                press(KeyCode::Backspace),
                press(KeyCode::Enter)
            ])
            .as_deref(),
            Some("anthropic/claude-opus-5")
        );
    }

    #[test]
    fn a_key_release_is_not_a_second_press() {
        // Terminals that report both would otherwise move twice per press.
        let mut release = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(
            drive(vec![
                press(KeyCode::Down),
                Event::Key(release),
                press(KeyCode::Enter)
            ])
            .as_deref(),
            Some("anthropic/claude-haiku-4-5")
        );
    }
}
