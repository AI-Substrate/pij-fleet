use std::path::{Path, PathBuf};

use pij_core::fleet::{
    Call, Corpus, Event, PriceTable, Seat, Session, Tokens, Turn, Window, analyze,
};

use super::*;

/// 2026-09-24T00:00:00Z.
const NOW: i64 = 1_790_208_000_000;
const DAY: i64 = 86_400_000;

fn args(folder: &str) -> FleetArgs {
    FleetArgs {
        folder: PathBuf::from(folder),
        with_worktrees: false,
        since: None,
        until: None,
        harness: None,
        out: Some(PathBuf::from("/tmp/out")),
        format: "jsonl".into(),
        include_content: false,
        anonymise: false,
        prep_target: None,
        utc_offset: None,
        threads: 4,
    }
}

fn no_worktrees(_: &Path) -> Result<Vec<PathBuf>, String> {
    Err("not called".into())
}

#[test]
fn worktree_porcelain_lists_every_worktree_path() {
    let porcelain = "worktree /repo/main\nHEAD abc\nbranch refs/heads/main\n\n\
                     worktree /repo/wt-a b\nHEAD def\ndetached\n\nworktree /repo/bare\nbare\n";
    assert_eq!(
        parse_worktrees(porcelain),
        [
            PathBuf::from("/repo/main"),
            PathBuf::from("/repo/wt-a b"),
            PathBuf::from("/repo/bare")
        ]
    );
}

#[test]
fn the_default_window_is_the_last_seven_days() {
    let plan = plan(
        &args("/work/demo"),
        NOW,
        600,
        &no_worktrees,
        Path::new("/r"),
    )
    .unwrap();
    assert_eq!(plan.window.until_ms, NOW);
    assert_eq!(plan.window.since_ms, NOW - 7 * DAY);
    assert_eq!(plan.window.utc_offset_min, 600);
    assert_eq!(plan.folders, [PathBuf::from("/work/demo")]);
}

#[test]
fn times_parse_as_rfc3339_local_dates_or_relative_spans() {
    assert_eq!(parse_time("2026-09-24T00:00:00Z", NOW, 0), Ok(NOW));
    assert_eq!(parse_time("2026-09-24T10:00:00+10:00", NOW, 0), Ok(NOW));
    assert_eq!(parse_time("2026-09-24", NOW, 600), Ok(NOW - 10 * 3_600_000));
    assert_eq!(parse_time("2d", NOW, 0), Ok(NOW - 2 * DAY));
    assert_eq!(parse_time("36h", NOW, 0), Ok(NOW - 36 * 3_600_000));
    assert!(parse_time("yesterday-ish", NOW, 0).is_err());
}

#[test]
fn with_worktrees_adds_every_worktree_once() {
    let mut a = args("/repo/main");
    a.with_worktrees = true;
    let worktrees = |_: &Path| {
        Ok(vec![
            PathBuf::from("/repo/main"),
            PathBuf::from("/repo/wt-a"),
        ])
    };
    let plan = plan(&a, NOW, 0, &worktrees, Path::new("/r")).unwrap();
    assert_eq!(
        plan.folders,
        [PathBuf::from("/repo/main"), PathBuf::from("/repo/wt-a")]
    );
}

/// Formats and flags P1 cannot honour are refused by name, never approximated.
#[test]
fn parquet_prep_target_and_a_relative_folder_are_refused_by_name() {
    let mut a = args("/work/demo");
    a.format = "parquet".into();
    let error = plan(&a, NOW, 0, &no_worktrees, Path::new("/r")).unwrap_err();
    assert!(error.starts_with("E-RS-FLEET-FORMAT"), "{error}");
    let mut a = args("/work/demo");
    a.prep_target = Some(PathBuf::from("/prep"));
    let error = plan(&a, NOW, 0, &no_worktrees, Path::new("/r")).unwrap_err();
    assert!(error.starts_with("E-RS-FLEET-PREP-TARGET"), "{error}");
    let error = plan(&args("rel/path"), NOW, 0, &no_worktrees, Path::new("/r")).unwrap_err();
    assert!(error.starts_with("E-RS-FLEET-FOLDER"), "{error}");
    let mut a = args("/work/demo");
    a.since = Some("2026-09-25".into());
    a.until = Some("2026-09-24".into());
    let error = plan(&a, NOW, 0, &no_worktrees, Path::new("/r")).unwrap_err();
    assert!(error.starts_with("E-RS-FLEET-WINDOW"), "{error}");
}

#[test]
fn without_out_the_report_lands_outside_any_repository() {
    let mut a = args("/work/demo");
    a.out = None;
    let plan = plan(&a, NOW, 0, &no_worktrees, Path::new("/home/me/.pij-rs")).unwrap();
    assert_eq!(
        plan.out,
        PathBuf::from("/home/me/.pij-rs/fleet-reports/demo-20260924T000000Z")
    );
}

#[test]
fn csv_cells_quote_commas_quotes_and_newlines() {
    assert_eq!(csv_cell("plain"), "plain");
    assert_eq!(csv_cell("a,b"), "\"a,b\"");
    assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(csv_cell("two\nlines"), "\"two\nlines\"");
}

/// A corpus full of things that must not leave the machine under `--anonymise`.
fn secret_corpus() -> Corpus {
    let source =
        "claude-code/default/-Users-secret-proj/0badc0de-1111-2222-3333-444455556666.jsonl";
    let call = |ts: i64, turn_no: i64, cw: u64| Call {
        source: source.into(),
        is_sub: false,
        ts_ms: ts,
        model: Some("claude-opus-5-5".into()),
        tokens: Tokens {
            input: 1,
            cw_1h: cw,
            cw_5m: 0,
            cache_read: 30_000,
            output: 10,
        },
        gap_ms: Some(4 * 3_600_000),
        turn_no,
        call_in_turn: 1,
    };
    Corpus {
        sessions: vec![Session {
            source: source.into(),
            harness: "claude-code".into(),
            session_id: Some("0badc0de-1111-2222-3333-444455556666".into()),
            parent_session_id: None,
            is_sub: false,
            cwd: Some("/Users/secret/proj".into()),
            first_ms: Some(NOW),
            last_ms: Some(NOW),
        }],
        calls: vec![
            call(NOW + 1_000, 1, 40_000),
            call(NOW + 9_000_000, 2, 40_000),
        ],
        turns: vec![
            Turn {
                source: source.into(),
                turn_no: 1,
                origin: "peer".into(),
                sender: Some("pij-hidden-boss".into()),
                pij_msg_id: Some("msg-feedface".into()),
                started_ms: Some(NOW),
                head: Some("TOP SECRET plan".into()),
            },
            Turn {
                source: source.into(),
                turn_no: 2,
                origin: "peer".into(),
                sender: Some("pij-secret-stoat".into()),
                pij_msg_id: None,
                started_ms: Some(NOW + 9_000_000),
                head: None,
            },
        ],
        events: vec![Event {
            source: source.into(),
            ts_ms: Some(NOW + 2_000),
            kind: "compaction".into(),
            subkind: None,
            trigger: Some("auto".into()),
            model: None,
            pre_tokens: Some(1),
            post_tokens: Some(1),
            duration_ms: None,
            last_context: None,
            gap_ms: None,
            resets_at: None,
        }],
        seats: vec![
            Seat {
                id: "pij-secret-stoat".into(),
                harness: "claude".into(),
                role: Some("o-prime".into()),
                folder: "/Users/secret/proj".into(),
                parent: Some("pij-hidden-boss".into()),
                spawned_ms: Some(NOW),
                ended_ms: None,
                sessions: vec!["0badc0de-1111-2222-3333-444455556666".into()],
            },
            Seat {
                id: "pij-hidden-boss".into(),
                harness: "omp".into(),
                role: Some("stream s07".into()),
                folder: "/Users/secret/other".into(),
                parent: None,
                spawned_ms: None,
                ended_ms: None,
                sessions: vec![],
            },
        ],
    }
}

fn written(anonymise: bool, include_content: bool, format: &str) -> (PathBuf, String) {
    let dir = pij_testkit::fresh_dir("pij-fleet-out");
    let mut corpus = secret_corpus();
    let mut a = args("/Users/secret/proj");
    a.out = Some(dir.clone());
    a.anonymise = anonymise;
    a.include_content = include_content;
    a.format = format.into();
    let plan = plan(&a, NOW + DAY, 0, &no_worktrees, Path::new("/r")).unwrap();
    if anonymise {
        anonymise_corpus(&mut corpus);
    }
    let window = Window {
        since_ms: NOW,
        until_ms: NOW + DAY,
        utc_offset_min: 0,
    };
    let report = analyze(&corpus, window, &PriceTable::default());
    write_output(&plan, &corpus, &report, &Facts::default()).expect("write");
    // The page is a fixed template: it must equal it byte for byte, so it can
    // carry no data, and only the data files are searched for leaks.
    assert_eq!(
        std::fs::read_to_string(dir.join("index.html")).unwrap(),
        write::PAGE
    );
    let mut all = String::new();
    for entry in walk(&dir) {
        if entry.file_name().is_some_and(|name| name != "index.html") {
            all.push_str(&std::fs::read_to_string(&entry).unwrap());
        }
    }
    (dir, all)
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// `--anonymise` must be safe to share: no seat name, path, session id, message
/// id or content anywhere in the output folder — in either table format.
#[test]
fn anonymised_output_leaks_no_name_path_id_or_content() {
    for format in ["jsonl", "csv"] {
        let (dir, all) = written(true, true, format);
        for secret in [
            "secret",
            "hidden",
            "stoat",
            "0badc0de",
            "feedface",
            "TOP SECRET",
            "/Users",
            "-Users",
            "s07",
        ] {
            assert!(!all.contains(secret), "{format}: `{secret}` leaked");
        }
        assert!(
            all.contains("Orchestrator A"),
            "{format}: roles and letters replace names"
        );
        assert!(all.contains("Worker A"), "{format}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The control: without `--anonymise` the same corpus does carry the names, so
/// the leak test above is not passing on an empty folder.
#[test]
fn the_unanonymised_control_carries_the_names() {
    let (dir, all) = written(false, false, "jsonl");
    assert!(all.contains("pij-secret-stoat"));
    assert!(
        !all.contains("TOP SECRET"),
        "content needs --include-content"
    );
    let _ = std::fs::remove_dir_all(dir);
    let (dir, all) = written(false, true, "jsonl");
    assert!(all.contains("TOP SECRET"));
    let _ = std::fs::remove_dir_all(dir);
}

/// The page works from a double-click: it loads report.js, whose one statement
/// assigns the same JSON as report.json.
#[test]
fn the_folder_holds_the_page_its_script_the_json_and_the_tables() {
    let (dir, _) = written(false, false, "csv");
    for file in [
        "index.html",
        "report.json",
        "report.js",
        "manifest.json",
        "tables/calls.csv",
        "tables/turns.csv",
        "tables/events.csv",
        "tables/compactions.csv",
        "tables/seats.csv",
        "tables/sessions.csv",
    ] {
        assert!(dir.join(file).is_file(), "missing {file}");
    }
    let json = std::fs::read_to_string(dir.join("report.json")).unwrap();
    let js = std::fs::read_to_string(dir.join("report.js")).unwrap();
    assert_eq!(js, format!("window.FLEET_REPORT = {json};\n"));
    let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
    assert!(html.contains("<script src=\"report.js\"></script>"));
    for external in ["http://", "https://", "//cdn"] {
        assert!(
            !html.replace("location.protocol", "").contains(external),
            "the page reaches the network: {external}"
        );
    }
    let calls = std::fs::read_to_string(dir.join("tables/calls.csv")).unwrap();
    assert!(calls.lines().next().unwrap().contains("in_window"));
    assert_eq!(calls.lines().count(), 3, "header + 2 calls");
    let _ = std::fs::remove_dir_all(dir);
}
