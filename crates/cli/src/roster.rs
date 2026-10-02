//! `pij list`'s human table (plan 160): who is on the roster, how big each
//! seat's context is, how long since its last call, whether its cache is warm,
//! and ❄ on a row the cold-wake guard would refuse right now.
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
    let mut rows: Vec<(bool, [String; 6])> = Vec::with_capacity(seats.len());
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
        let cold = seat["sizeColumns"][3].as_str() == Some("❄");
        rows.push((
            cold,
            [
                id,
                text("harness"),
                text("state"),
                column(0),
                column(1),
                column(2),
            ],
        ));
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
    for (cold, row) in &rows {
        lines.push(line(if *cold { "❄ " } else { "  " }, row));
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
