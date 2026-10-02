//! The output folder: tables, report.json, report.js, manifest.json, index.html.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::Path;

use pij_core::fleet::{Corpus, REPORT_VERSION, Report, Tokens, source_labels};
use serde_json::{Value, json};

use super::{Facts, Format, Plan};

/// The page: static, no network, renders `window.FLEET_REPORT`.
pub(super) const PAGE: &str = include_str!("page.html");

/// One CSV cell, quoted when it must be.
pub fn csv_cell(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => csv_cell(text),
        other => csv_cell(&other.to_string()),
    }
}

/// One table: column names and rows of values in that order.
struct Table {
    name: &'static str,
    columns: &'static [&'static str],
    rows: Vec<Vec<Value>>,
}

fn write_table(dir: &Path, format: Format, table: &Table) -> io::Result<String> {
    let mut text = String::new();
    let file = match format {
        Format::Jsonl => {
            for row in &table.rows {
                let object: serde_json::Map<String, Value> = table
                    .columns
                    .iter()
                    .zip(row)
                    .map(|(column, value)| ((*column).to_string(), value.clone()))
                    .collect();
                text.push_str(&Value::Object(object).to_string());
                text.push('\n');
            }
            format!("{}.jsonl", table.name)
        }
        Format::Csv => {
            text.push_str(&table.columns.join(","));
            text.push('\n');
            for row in &table.rows {
                text.push_str(&row.iter().map(cell).collect::<Vec<_>>().join(","));
                text.push('\n');
            }
            format!("{}.csv", table.name)
        }
    };
    fs::write(dir.join(&file), text)?;
    Ok(format!("tables/{file}"))
}

fn iso(ts_ms: Option<i64>) -> Value {
    ts_ms
        .and_then(|ms| {
            time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
        })
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .map_or(Value::Null, Value::String)
}

fn tokens_json(tokens: &Tokens) -> [Value; 5] {
    [
        json!(tokens.input),
        json!(tokens.cw_1h),
        json!(tokens.cw_5m),
        json!(tokens.cache_read),
        json!(tokens.output),
    ]
}

fn tables(plan: &Plan, corpus: &Corpus) -> Vec<Table> {
    let window = plan.window;
    let labels = source_labels(corpus);
    let label = |source: &str| {
        labels
            .get(source)
            .cloned()
            .map_or(Value::Null, Value::String)
    };
    let harness: HashMap<&str, &str> = corpus
        .sessions
        .iter()
        .map(|s| (s.source.as_str(), s.harness.as_str()))
        .collect();

    let calls = corpus
        .calls
        .iter()
        .map(|c| {
            let mut row = vec![
                json!(c.source),
                label(&c.source),
                json!(harness.get(c.source.as_str())),
                json!(c.model),
                json!(c.ts_ms),
                iso(Some(c.ts_ms)),
            ];
            row.extend(tokens_json(&c.tokens));
            row.extend([
                json!(c.context()),
                json!(c.gap_ms),
                json!(c.turn_no),
                json!(c.call_in_turn),
                json!(c.cold()),
                json!(c.idle()),
                json!(c.ttl_ms() / 1_000),
                json!(window.contains(c.ts_ms)),
            ]);
            row
        })
        .collect();

    #[derive(Default)]
    struct Agg {
        calls: u64,
        tokens: Tokens,
        cold: bool,
        first_ts: Option<i64>,
    }
    let mut per_turn: BTreeMap<(&str, i64), Agg> = BTreeMap::new();
    for c in &corpus.calls {
        let agg = per_turn.entry((c.source.as_str(), c.turn_no)).or_default();
        agg.calls += 1;
        agg.tokens.add(&c.tokens);
        agg.cold |= c.cold();
        agg.first_ts = Some(agg.first_ts.map_or(c.ts_ms, |ts| ts.min(c.ts_ms)));
    }
    let turns = corpus
        .turns
        .iter()
        .map(|t| {
            let agg = per_turn.get(&(t.source.as_str(), t.turn_no));
            let first = agg.and_then(|a| a.first_ts).or(t.started_ms);
            let mut row = vec![
                json!(t.source),
                label(&t.source),
                json!(t.turn_no),
                json!(t.origin),
                json!(t.sender),
                json!(t.pij_msg_id),
                json!(t.started_ms),
                json!(agg.map_or(0, |a| a.calls)),
            ];
            row.extend(tokens_json(&agg.map(|a| a.tokens).unwrap_or_default()));
            row.extend([
                json!(agg.is_some_and(|a| a.cold)),
                json!(first.is_some_and(|ts| window.contains(ts))),
                if plan.include_content && !plan.anonymise {
                    json!(t.head)
                } else {
                    Value::Null
                },
            ]);
            row
        })
        .collect();

    let event_row = |e: &pij_core::fleet::Event| {
        vec![
            json!(e.source),
            label(&e.source),
            json!(e.ts_ms),
            iso(e.ts_ms),
            json!(e.kind),
            json!(e.subkind),
            json!(e.trigger),
            json!(e.model),
            json!(e.pre_tokens),
            json!(e.post_tokens),
            json!(e.duration_ms),
            json!(e.last_context),
            json!(e.gap_ms),
            json!(e.resets_at),
            json!(e.ts_ms.is_some_and(|ts| window.contains(ts))),
        ]
    };
    let events = corpus.events.iter().map(event_row).collect();
    let compactions = corpus
        .events
        .iter()
        .filter(|e| e.kind == "compaction")
        .map(event_row)
        .collect();

    let seats = corpus
        .seats
        .iter()
        .map(|s| {
            vec![
                json!(s.id),
                json!(s.harness),
                json!(s.role),
                if s.folder.is_empty() {
                    Value::Null
                } else {
                    json!(s.folder)
                },
                json!(s.parent),
                json!(s.spawned_ms),
                json!(s.ended_ms),
                json!(s.sessions.join(";")),
            ]
        })
        .collect();

    let sessions = corpus
        .sessions
        .iter()
        .map(|s| {
            let name = labels.get(&s.source).cloned().unwrap_or_default();
            vec![
                json!(s.source),
                json!(s.harness),
                json!(s.session_id),
                json!(s.parent_session_id),
                json!(s.is_sub),
                json!(s.cwd),
                json!(s.first_ms),
                json!(s.last_ms),
                json!(name),
                json!(name.starts_with("session ")),
            ]
        })
        .collect();

    const EVENT_COLUMNS: &[&str] = &[
        "session",
        "seat",
        "ts_ms",
        "ts",
        "kind",
        "subkind",
        "trigger",
        "model",
        "pre_tokens",
        "post_tokens",
        "duration_ms",
        "last_context",
        "gap_ms",
        "resets_at",
        "in_window",
    ];
    vec![
        Table {
            name: "calls",
            columns: &[
                "session",
                "seat",
                "harness",
                "model",
                "ts_ms",
                "ts",
                "input",
                "cw_1h",
                "cw_5m",
                "cache_read",
                "output",
                "context",
                "gap_ms",
                "turn_no",
                "call_in_turn",
                "cold",
                "idle",
                "cache_ttl_s",
                "in_window",
            ],
            rows: calls,
        },
        Table {
            name: "turns",
            columns: &[
                "session",
                "seat",
                "turn_no",
                "origin",
                "sender",
                "pij_msg_id",
                "started_ms",
                "calls",
                "input",
                "cw_1h",
                "cw_5m",
                "cache_read",
                "output",
                "cold",
                "in_window",
                "head",
            ],
            rows: turns,
        },
        Table {
            name: "events",
            columns: EVENT_COLUMNS,
            rows: events,
        },
        Table {
            name: "compactions",
            columns: EVENT_COLUMNS,
            rows: compactions,
        },
        Table {
            name: "seats",
            columns: &[
                "seat",
                "harness",
                "role",
                "folder",
                "parent",
                "spawned_ms",
                "ended_ms",
                "harness_sessions",
            ],
            rows: seats,
        },
        Table {
            name: "sessions",
            columns: &[
                "session",
                "harness",
                "session_id",
                "parent_session_id",
                "is_sub",
                "cwd",
                "first_ms",
                "last_ms",
                "seat",
                "unseated",
            ],
            rows: sessions,
        },
    ]
}

fn manifest(
    plan: &Plan,
    corpus: &Corpus,
    report: &Report,
    facts: &Facts,
    counts: &BTreeMap<&str, usize>,
) -> Value {
    let mut all = Tokens::default();
    for call in &corpus.calls {
        all.add(&call.tokens);
    }
    let folders: Value = if plan.anonymise {
        json!({ "count": plan.folders.len() })
    } else {
        json!(plan.folders)
    };
    json!({
        "folder": if plan.anonymise { Value::Null } else { json!(plan.folder) },
        "folders": folders,
        "window": {
            "since_ms": plan.window.since_ms,
            "until_ms": plan.window.until_ms,
            "since": iso(Some(plan.window.since_ms)),
            "until": iso(Some(plan.window.until_ms)),
            "utc_offset_min": plan.window.utc_offset_min,
        },
        "generated": iso(Some(facts.generated_ms)),
        "versions": {
            "pij": facts.pij_version,
            "report": REPORT_VERSION,
            "unisphere_table_schema": facts.prep_table_schema,
            "unisphere_policies": facts.prep_policies,
            "pij_store_schema": facts.store_schema,
        },
        "options": {
            "format": plan.format,
            "anonymised": plan.anonymise,
            "include_content": plan.include_content && !plan.anonymise,
            "harnesses": plan.harnesses,
        },
        "prep": {
            "bytes_read": facts.prep_bytes_read,
            "sources_discovered": facts.prep_sources.0,
            "sources_folded": facts.prep_sources.1,
        },
        "row_counts": counts,
        "token_totals": { "all_rows": all, "in_window": report.totals.tokens },
        "warnings": if plan.anonymise { json!({ "count": facts.warnings.len() }) } else { json!(facts.warnings) },
    })
}

/// Write the whole folder; returns the files written, relative to it.
///
/// # Errors
/// The first IO error.
pub fn write_output(
    plan: &Plan,
    corpus: &Corpus,
    report: &Report,
    facts: &Facts,
) -> io::Result<Vec<String>> {
    let dir = &plan.out;
    fs::create_dir_all(dir.join("tables"))?;
    let mut written = Vec::new();
    let mut counts = BTreeMap::new();
    for table in tables(plan, corpus) {
        counts.insert(table.name, table.rows.len());
        written.push(write_table(&dir.join("tables"), plan.format, &table)?);
    }
    let json = serde_json::to_string(report).map_err(io::Error::other)?;
    fs::write(dir.join("report.json"), &json)?;
    fs::write(
        dir.join("report.js"),
        format!("window.FLEET_REPORT = {json};\n"),
    )?;
    let manifest = manifest(plan, corpus, report, facts, &counts);
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(io::Error::other)?,
    )?;
    fs::write(dir.join("index.html"), PAGE)?;
    written.extend(["report.json", "report.js", "manifest.json", "index.html"].map(String::from));
    Ok(written)
}
