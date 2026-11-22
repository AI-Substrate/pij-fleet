//! `pij list`'s human table (plan 160): who is on the roster, how big each
//! seat's context is, how long since its last call, whether its cache is warm,
//! ❄ on a row the cold-wake guard would refuse right now, and ⚠ on one it
//! would warn about (F214). A seat stalled on
//! its harness's own dialog shows `blocked on a dialog (<key>)` (plan 219); a
//! native Copilot seat whose receiver lease lapsed adds `receiver lapsed <age>`
//! to its state (F195).
//!
//! The daemon derives and renders the size columns (`sizeColumns`), so this
//! only lays them out. The TS shim renders the same table from the same JSON;
//! the golden `fixtures/golden/cli/list-sized.{json,txt}` pins both.

use serde_json::Value;

const HEADERS: [&str; 6] = ["SEAT", "HARNESS", "STATE", "CTX", "IDLE", "CACHE"];

/// The table for a `/v1/seats?sizes=true` payload (`{seats, unavailable}`).
/// A row without size columns (a remote seat) shows `-`.
pub fn render_table(data: &Value) -> String {
    let seats = data["seats"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rows: Vec<(&str, [String; 6])> = Vec::with_capacity(seats.len());
    for seat in seats {
        let text = |key: &str| seat[key].as_str().unwrap_or("-").to_string();
        let id = match seat["machine"].as_str() {
            Some(machine) if seat.get("sizeColumns").is_none() => {
                format!("{}@{machine}", text("id"))
            }
            _ => text("id"),
        };
        let column = |index: usize| {
            seat["sizeColumns"][index]
                .as_str()
                .unwrap_or("-")
                .to_string()
        };
        // The marker is one of the guard's two, never other text.
        let mark = match seat["sizeColumns"][3].as_str() {
            Some("❄") => "❄ ",
            Some("⚠") => "⚠ ",
            _ => "  ",
        };
        // Plan 219: a seat on its harness's own dialog reads working; say so.
        // F195: a lapsed native receiver is appended, since the process runs on.
        let state = match seat["dialog"]["key"].as_str() {
            Some(key) => format!("blocked on a dialog ({key})"),
            None => text("state"),
        };
        let state = match seat["nativeReceiver"].as_str() {
            Some(lapse) => format!("{state}, {lapse}"),
            None => state,
        };
        // F280: a seat launched with an alt shows it beside its harness.
        let harness = match seat["alt"]["name"].as_str() {
            Some(alt) => format!("{} alt:{alt}", text("harness")),
            None => text("harness"),
        };
        rows.push((mark, [id, harness, state, column(0), column(1), column(2)]));
    }
    let mut widths = HEADERS.map(|header| header.chars().count());
    for (_, row) in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |mark: &str, cells: &[String; 6]| {
        let mut text = mark.to_string();
        for (index, (cell, width)) in cells.iter().zip(widths).enumerate() {
            if index + 1 == cells.len() {
                text.push_str(cell);
            } else {
                text.push_str(&format!("{cell:<width$}  "));
            }
        }
        text.trim_end().to_string()
    };
    let mut lines = vec![line("  ", &HEADERS.map(str::to_string))];
    for (mark, row) in &rows {
        lines.push(line(mark, row));
    }
    for peer in data["unavailable"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        lines.push(format!(
            "unavailable: {} ({})",
            peer["machine"].as_str().unwrap_or("?"),
            peer["reason"].as_str().unwrap_or("?")
        ));
    }
    lines.join("\n")
}
