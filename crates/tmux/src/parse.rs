use pij_core::error::{PijError, Result};
use pij_core::model::Pane;

pub(crate) const FIELD_SEPARATOR: char = '|';
pub(crate) const LIST_FORMAT: &str =
    "#{pane_id}|#{session_name}|#{window_name}|#{cursor_x}|#{cursor_y}|#{pane_title}";

pub(crate) fn parse_panes(output: &str) -> Result<Vec<Pane>> {
    output
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.is_empty())
        .map(|(index, line)| parse_pane(line, index + 1))
        .collect()
}

pub(crate) fn parse_one_pane(output: &str, operation: &str) -> Result<Pane> {
    let panes = parse_panes(output)?;
    match panes.as_slice() {
        [pane] => Ok(pane.clone()),
        _ => Err(adapter_error(format!(
            "{operation} returned {} pane rows, expected exactly one — run `tmux list-panes -a` to inspect the target",
            panes.len()
        ))),
    }
}

fn parse_pane(line: &str, line_number: usize) -> Result<Pane> {
    // Historical fixtures carry the original four-field wire shape. Production
    // now puts cursor X/Y before the title; the title remains last because it is
    // user-controlled and may contain our separator.
    let mut fields = line.splitn(4, FIELD_SEPARATOR);
    let id = required(&mut fields, "pane id", line_number)?;
    let session = required(&mut fields, "session", line_number)?;
    let window = required(&mut fields, "window", line_number)?;
    let Some(remainder) = fields.next() else {
        return Err(adapter_error(format!(
            "list-panes row {line_number} has no title field — update pij-tmux for this tmux wire shape"
        )));
    };

    if !id.starts_with('%') {
        return Err(adapter_error(format!(
            "list-panes row {line_number} has invalid pane id {id:?} — run `tmux list-panes -a` to inspect the source"
        )));
    }

    let mut cursor_and_title = remainder.splitn(3, FIELD_SEPARATOR);
    let first = cursor_and_title.next().unwrap_or_default();
    let second = cursor_and_title.next();
    let third = cursor_and_title.next();
    let (cursor, title) = match (second, third) {
        (Some(y), Some(title)) => (first.parse::<u32>().ok().zip(y.parse::<u32>().ok()), title),
        _ => (None, remainder),
    };
    Ok(Pane {
        id: id.to_string(),
        session: session.to_string(),
        window: window.to_string(),
        title: title.to_string(),
        cursor_x: cursor.map(|(x, _)| x),
        cursor_y: cursor.map(|(_, y)| y),
    })
}

fn required<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    name: &str,
    line_number: usize,
) -> Result<&'a str> {
    match fields.next() {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(adapter_error(format!(
            "list-panes row {line_number} has no {name} — update pij-tmux for this tmux wire shape"
        ))),
    }
}

fn adapter_error(message: String) -> PijError {
    PijError::Adapter {
        adapter: "tmux".to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_panes;

    const EMPTY_TITLE: &str =
        include_str!("../../testkit/fixtures/tmux/list-panes-empty-title.txt");
    const HOSTILE_TITLE: &str =
        include_str!("../../testkit/fixtures/tmux/list-panes-hostile-title.txt");

    #[test]
    fn captured_empty_title_is_present_not_missing() {
        let panes = parse_panes(EMPTY_TITLE).expect("captured tmux output");
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].id, "%320");
        assert_eq!(panes[0].session, "pij-w1-fixture-c84a1");
        assert_eq!(panes[0].window, "normal");
        assert_eq!(panes[0].title, "");
        assert_eq!((panes[0].cursor_x, panes[0].cursor_y), (None, None));
    }

    #[test]
    fn title_keeps_the_field_separator_because_the_final_field_is_unsplit() {
        let panes = parse_panes(HOSTILE_TITLE).expect("captured hostile tmux output");
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].id, "%320");
        assert_eq!(panes[0].title, "human · title | nested");
    }

    #[test]
    fn malformed_cursor_is_unknown_as_a_pair() {
        let panes = parse_panes("%1|s|w|7|not-a-row|title").expect("structural pane row");
        assert_eq!((panes[0].cursor_x, panes[0].cursor_y), (None, None));
    }
}
