use super::*;

const HOUR: i64 = 3_600_000;
/// 2026-09-24 00:00 AEST.
const T0: i64 = 1_790_172_000_000;
const AEST: i32 = 600;

fn window() -> Window {
    Window {
        since_ms: T0,
        until_ms: T0 + 24 * HOUR,
        utc_offset_min: AEST,
    }
}

fn tokens(input: u64, cw_1h: u64, cache_read: u64, output: u64) -> Tokens {
    Tokens {
        input,
        cw_1h,
        cw_5m: 0,
        cache_read,
        output,
    }
}

fn call(source: &str, ts_ms: i64, gap_ms: Option<i64>, turn_no: i64, t: Tokens) -> Call {
    Call {
        source: source.into(),
        is_sub: false,
        ts_ms,
        model: Some("claude-opus-5-5".into()),
        tokens: t,
        gap_ms,
        turn_no,
        call_in_turn: 1,
    }
}

fn turn(source: &str, turn_no: i64, origin: &str, sender: Option<&str>) -> Turn {
    Turn {
        source: source.into(),
        turn_no,
        origin: origin.into(),
        sender: sender.map(Into::into),
        pij_msg_id: None,
        started_ms: None,
        head: None,
    }
}

fn session(source: &str, session_id: &str) -> Session {
    Session {
        source: source.into(),
        harness: "claude-code".into(),
        session_id: Some(session_id.into()),
        parent_session_id: None,
        is_sub: false,
        cwd: Some("/work/demo".into()),
        first_ms: None,
        last_ms: None,
    }
}

/// Number the calls of each turn 1, 2, 3, … in order.
fn number(mut calls: Vec<Call>) -> Vec<Call> {
    let mut last = (String::new(), -1);
    let mut n = 0;
    for c in &mut calls {
        if (c.source.clone(), c.turn_no) != last {
            last = (c.source.clone(), c.turn_no);
            n = 0;
        }
        n += 1;
        c.call_in_turn = n;
    }
    calls
}

#[test]
fn a_call_is_cold_only_above_20k_when_it_wrote_half_its_context() {
    let at = |cw: u64, read: u64| call("s", T0, None, 1, tokens(0, cw, read, 0)).cold();
    assert!(at(10_001, 10_000), "20,001 context, writes >= half");
    assert!(!at(10_000, 10_000), "exactly 20k is not above the floor");
    assert!(
        at(15_000, 15_000),
        "writes of exactly half the context are cold"
    );
    assert!(!at(10_000, 10_002), "writes under half are warm");
}

#[test]
fn idle_means_the_gap_outlived_the_cache_and_a_first_call_is_never_idle() {
    let mut c = call("s", T0, Some(HOUR + 1), 1, Tokens::default());
    assert!(c.idle());
    c.gap_ms = Some(HOUR);
    assert!(!c.idle(), "exactly the TTL is still warm");
    c.is_sub = true;
    c.gap_ms = Some(300_001);
    assert!(c.idle(), "a subagent's cache lives 5 minutes");
    c.gap_ms = None;
    assert!(!c.idle());
}

#[test]
fn bucket_keys_read_the_report_clock() {
    assert_eq!(bucket_key(T0, AEST, 60), "2026-09-24T00");
    assert_eq!(bucket_key(T0 - 1, AEST, 60), "2026-09-23T23");
    assert_eq!(bucket_key(T0 + 7 * 60_000, AEST, 5), "2026-09-24T00:05");
    assert_eq!(
        day_key(T0, 0),
        "2026-09-23",
        "the same instant is the 23rd in UTC"
    );
    assert_eq!(
        bucket_key(951_782_400_000, 0, 60),
        "2000-02-29T00",
        "leap day"
    );
}

#[test]
fn prices_apply_per_class_and_reads_free_drops_only_reads() {
    let t = Tokens {
        input: 1_000_000,
        cw_1h: 1_000_000,
        cw_5m: 1_000_000,
        cache_read: 1_000_000,
        output: 1_000_000,
    };
    let p = Prices::opus_5_5();
    assert!((t.usd(&p) - 37.2).abs() < 1e-9);
    assert!((t.usd_reads_free(&p) - 37.0).abs() < 1e-9);
}

/// The split is a partition: every call token lands in exactly one component.
#[test]
fn the_spend_split_partitions_every_call_token() {
    let corpus = Corpus {
        sessions: vec![session("a", "sa")],
        calls: number(vec![
            call("a", T0 + HOUR, None, 1, tokens(10, 40_000, 0, 500)),
            call(
                "a",
                T0 + HOUR + 60_000,
                Some(60_000),
                1,
                tokens(3, 900, 40_000, 200),
            ),
        ]),
        turns: vec![turn("a", 1, "human", None)],
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    let c = report.components;
    let mut sum = Tokens::default();
    for part in [
        c.warm_reads,
        c.cold_rewrite,
        c.incremental_writes,
        c.output_cost,
    ] {
        sum.add(&part);
    }
    assert_eq!(sum, report.totals.tokens);
    assert_eq!(
        c.cold_rewrite.cw_1h, 40_000,
        "the first call rebuilt the cache"
    );
    assert_eq!(c.incremental_writes.cw_1h, 900);
    assert_eq!(report.totals.cold_calls, 1);
}

/// A session with any call in the window brings its rows; aggregates count only
/// the calls inside it.
#[test]
fn aggregates_count_only_calls_inside_the_window() {
    let corpus = Corpus {
        sessions: vec![session("a", "sa")],
        calls: number(vec![
            call("a", T0 - HOUR, None, 1, tokens(1, 0, 0, 1)),
            call("a", T0 + HOUR, Some(2 * HOUR), 1, tokens(1, 0, 0, 1)),
            call("a", T0 + 30 * HOUR, Some(29 * HOUR), 1, tokens(1, 0, 0, 1)),
        ]),
        turns: vec![turn("a", 1, "human", None)],
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    assert_eq!(report.totals.calls, 1);
    assert_eq!(report.hourly.len(), 24);
    assert_eq!(report.hourly[1].calls, 1);
}

/// The RCA's status turn: a message-opened turn of three calls or fewer. A cold
/// wake it opened after the cache expired was avoidable.
#[test]
fn status_turns_and_their_avoidable_cold_wakes() {
    let wake = tokens(0, 100_000, 0, 50);
    let warm = tokens(0, 500, 100_000, 50);
    let calls = number(vec![
        // turn 1: typed by the user, warm work
        call("a", T0 + HOUR, None, 1, tokens(0, 100_000, 0, 10)),
        call("a", T0 + HOUR + 1_000, Some(1_000), 1, warm),
        // turn 2: a peer ack after 2 h idle: a status turn and an avoidable cold wake
        call("a", T0 + 3 * HOUR, Some(2 * HOUR), 2, wake),
        // turn 3: a peer instruction of 4 calls after 2 h idle: real work
        call("a", T0 + 5 * HOUR, Some(2 * HOUR), 3, wake),
        call("a", T0 + 5 * HOUR + 1_000, Some(1_000), 3, warm),
        call("a", T0 + 5 * HOUR + 2_000, Some(1_000), 3, warm),
        call("a", T0 + 5 * HOUR + 3_000, Some(1_000), 3, warm),
    ]);
    let corpus = Corpus {
        sessions: vec![session("a", "sa")],
        calls,
        turns: vec![
            turn("a", 1, "human", None),
            turn("a", 2, "peer", Some("pij-boss")),
            turn("a", 3, "peer", Some("pij-boss")),
        ],
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    let st = &report.status_turns;
    assert_eq!(st.turns, 1);
    assert_eq!(st.idle_cold_wakes, 2);
    assert_eq!(st.avoidable_cold_wakes, 1);
    assert_eq!(st.work_cold_wakes, 1);
    assert_eq!(st.cold_tokens.cw_1h, 100_000);
    let p = Prices::opus_5_5();
    let total = report.totals.tokens.usd(&p);
    assert!((st.share - 100.0 * wake.usd(&p) / total).abs() < 1e-9);
    assert!(st.share_no_cold < st.share);
    // Without the ack, turn 3's wake would have been the same cold wake: saving > 0.
    assert!(st.replay_saving_share > 0.0, "{st:?}");
    assert_eq!(report.key_figures.avoidable_cold_wakes, 1);
    let peer = report
        .triggers
        .iter()
        .find(|row| row.trigger == "peer message")
        .expect("peer row");
    assert_eq!((peer.turns, peer.calls, peer.cold_wakes), (2, 5, 2));
}

/// Seats come from pij's store by harness session id; a session with no seat is
/// reported as unseated, never dropped.
#[test]
fn sessions_join_seats_by_harness_session_and_unseated_ones_stay() {
    let corpus = Corpus {
        sessions: vec![session("a", "sa"), session("b", "sb")],
        calls: number(vec![
            call("a", T0 + HOUR, None, 1, tokens(0, 30_000, 0, 1)),
            call("b", T0 + HOUR, None, 1, tokens(0, 30_000, 0, 1)),
        ]),
        turns: vec![turn("a", 1, "human", None), turn("b", 1, "human", None)],
        seats: vec![Seat {
            id: "pij-able-stoat".into(),
            harness: "claude".into(),
            role: Some("o-prime".into()),
            folder: "/work/demo".into(),
            parent: None,
            spawned_ms: None,
            ended_ms: None,
            sessions: vec!["old".into(), "sa".into()],
        }],
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    assert_eq!(report.totals.seated_sessions, 1);
    assert_eq!(report.totals.unseated_sessions, 1);
    let ids: Vec<&str> = report.graph.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&"pij-able-stoat"), "{ids:?}");
    assert!(ids.contains(&"session sb"), "{ids:?}");
    let stoat = report
        .graph
        .nodes
        .iter()
        .find(|n| n.id == "pij-able-stoat")
        .unwrap();
    assert_eq!(stoat.role.as_deref(), Some("o-prime"));
    assert!(stoat.seated);
}

/// Reads of a run that grows by g per call from ~0 sum to ~C²/2g: exponent 2.
/// Writes and output grow linearly: exponent 1. (A run that starts large is
/// affine in its writes, which is why real runs measure ~1.2.)
#[test]
fn the_square_rule_measures_reads_quadratic_and_writes_linear() {
    let mut calls = Vec::new();
    let mut sessions = Vec::new();
    for run in 0..5 {
        let source = format!("r{run}");
        sessions.push(session(&source, &format!("s{run}")));
        let mut ctx = 2_000u64;
        let mut ts = T0 + HOUR;
        let mut first = true;
        while ctx < 850_000 {
            let t = if first {
                tokens(0, ctx, 0, 500)
            } else {
                tokens(0, 2_000, ctx - 2_000, 500)
            };
            calls.push(call(
                &source,
                ts,
                if first { None } else { Some(1_000) },
                1,
                t,
            ));
            first = false;
            ctx += 2_000;
            ts += 1_000;
        }
    }
    let corpus = Corpus {
        sessions,
        calls: number(calls),
        turns: (0..5)
            .map(|run| turn(&format!("r{run}"), 1, "human", None))
            .collect(),
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    let sq = &report.square_rule;
    assert_eq!(sq.runs, 5);
    let reads = sq.exponents["reads"];
    let writes = sq.exponents["writes"];
    assert!((reads - 2.0).abs() < 0.2, "reads exponent {reads}");
    assert!((writes - 1.0).abs() < 0.05, "writes exponent {writes}");
    assert_eq!(sq.milestones.len(), 4);
}

/// The page draws growth curves, not every call: a curve keeps a point per
/// 5k of new peak context (plus its last), so a big fleet's report stays light.
#[test]
fn growth_curves_are_thinned_to_one_point_per_5k_of_context() {
    let mut calls = Vec::new();
    let mut ctx = 2_000u64;
    let mut ts = T0 + HOUR;
    while ctx < 850_000 {
        let first = ctx == 2_000;
        let t = if first {
            tokens(0, ctx, 0, 500)
        } else {
            tokens(0, 1_000, ctx - 1_000, 500)
        };
        calls.push(call("r", ts, if first { None } else { Some(1_000) }, 1, t));
        ctx += 1_000;
        ts += 1_000;
    }
    let corpus = Corpus {
        sessions: vec![session("r", "s")],
        calls: number(calls),
        turns: vec![turn("r", 1, "human", None)],
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    let points = &report.context_cost.runs[0].points;
    assert!(
        points.len() <= 850_000 / 5_000 + 2,
        "{} points",
        points.len()
    );
    for pair in points.windows(2).take(points.len().saturating_sub(2)) {
        assert!(pair[1][0] - pair[0][0] >= 5_000.0, "{pair:?}");
    }
    let last = points.last().unwrap();
    assert_eq!(last[0], 849_000.0, "the run's peak is kept");
}

/// Compactions are simulated: a cold one re-reads the history at the 5-minute
/// write rate, a warm one as cached reads; the summary is output (capped at 30k).
#[test]
fn compactions_are_simulated_by_their_cache_state() {
    let event = |gap_ms: Option<i64>| Event {
        source: "a".into(),
        ts_ms: Some(T0 + HOUR),
        kind: "compaction".into(),
        subkind: None,
        trigger: Some("auto".into()),
        model: None,
        pre_tokens: Some(400_000),
        post_tokens: Some(50_000),
        duration_ms: None,
        last_context: None,
        gap_ms,
        resets_at: None,
    };
    let corpus = Corpus {
        sessions: vec![session("a", "sa")],
        events: vec![event(Some(60_000)), event(Some(2 * HOUR)), event(None)],
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    let sim = report.components.compaction_sim;
    assert_eq!(sim.cache_read, 400_000);
    assert_eq!(sim.cw_5m, 800_000);
    assert_eq!(sim.output, 90_000);
    assert_eq!(report.compactions.count, 3);
    assert_eq!(report.compactions.cold, 2);
}

#[test]
fn an_empty_corpus_is_a_report_of_zeros_not_a_panic() {
    let report = analyze(&Corpus::default(), window(), &PriceTable::default());
    assert_eq!(report.totals.calls, 0);
    assert_eq!(report.key_figures.total_usd, 0.0);
    assert_eq!(report.version, REPORT_VERSION);
}

/// A growth run carries what the page needs to explain it on hover: who, when,
/// what it cost, its turns, status turns and cold wakes, markers where they
/// happened, and the same run replayed without its status turns.
#[test]
fn a_growth_run_carries_its_story_for_the_hover() {
    let mut calls = Vec::new();
    let mut ctx = 2_000u64;
    let mut ts = T0 + HOUR;
    let warm = |ctx: u64| tokens(0, 2_000, ctx - 2_000, 300);
    // turn 1: typed work from 2k to 250k
    calls.push(call("r", ts, None, 1, tokens(0, ctx, 0, 300)));
    while ctx < 250_000 {
        ctx += 2_000;
        ts += 1_000;
        calls.push(call("r", ts, Some(1_000), 1, warm(ctx)));
    }
    // turn 2: a warm ack a minute later (a status turn, drawn as a dot)
    ts += 60_000;
    ctx += 2_000;
    calls.push(call("r", ts, Some(60_000), 2, warm(ctx)));
    // turn 3: an ack two hours later wakes the seat cold (avoidable; a red
    // triangle, not also a dot, as the RCA draws it)
    ts += 2 * HOUR;
    calls.push(call("r", ts, Some(2 * HOUR), 3, tokens(0, ctx, 0, 50)));
    // turn 4: real work by message, on to 500k
    while ctx < 500_000 {
        ctx += 2_000;
        ts += 1_000;
        calls.push(call("r", ts, Some(1_000), 4, warm(ctx)));
    }
    let corpus = Corpus {
        sessions: vec![session("r", "s")],
        calls: number(calls),
        turns: vec![
            turn("r", 1, "human", None),
            turn("r", 2, "peer", Some("pij-boss")),
            turn("r", 3, "peer", Some("pij-boss")),
            turn("r", 4, "peer", Some("pij-boss")),
        ],
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    let run = &report.context_cost.runs[0];
    assert_eq!(run.seat, "session s");
    assert_eq!(run.harness, "claude-code");
    assert_eq!(
        run.session_id.as_deref(),
        Some("s"),
        "to go back to the transcript"
    );
    assert_eq!(run.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!((run.start_ms, run.end_ms), (T0 + HOUR, ts));
    assert_eq!((run.turns, run.message_turns, run.status_turns), (4, 3, 2));
    assert_eq!((run.cold_wakes, run.avoidable_cold_wakes), (1, 1));
    let kinds: Vec<&str> = run.markers.iter().map(|m| m.kind.as_str()).collect();
    assert_eq!(kinds, ["status", "cold_avoidable"]);
    let with = run.points.last().unwrap()[1];
    let without = run.replay.last().unwrap()[1];
    assert!(
        without < with,
        "the replay without the ack is cheaper: {without} vs {with}"
    );
    assert!((run.usd - with).abs() < 1e-9);
    let model = &report.context_cost.model;
    assert!(
        model.len() > 10 && model.windows(2).all(|w| w[1][1] >= w[0][1]),
        "{model:?}"
    );
}

/// The headline status-turn figure turns on its boundary: a message-opened turn
/// of exactly three calls is a status turn; four calls is work.
#[test]
fn a_three_call_message_turn_is_a_status_turn_and_four_is_not() {
    let mut calls = Vec::new();
    for (turn_no, n) in [(1, 3), (2, 4)] {
        for i in 0..n {
            calls.push(call(
                "a",
                T0 + HOUR * turn_no + i * 1_000,
                Some(1_000),
                turn_no,
                tokens(0, 100, 30_000, 10),
            ));
        }
    }
    let corpus = Corpus {
        sessions: vec![session("a", "sa")],
        calls: number(calls),
        turns: vec![
            turn("a", 1, "peer", Some("p")),
            turn("a", 2, "peer", Some("p")),
        ],
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    assert_eq!(report.status_turns.turns, 1);
}

/// The hub graph: pij message counts for every pair touching a seat in scope,
/// primes marked as hubs, and a counterpart with no local seat (another
/// machine) kept and marked as not local. Pairs that touch nothing in scope stay out.
#[test]
fn the_message_graph_keeps_pairs_touching_the_scope_and_marks_primes_and_remotes() {
    let seat = |id: &str, session: &str| Seat {
        id: id.into(),
        harness: "claude".into(),
        role: None,
        folder: "/work/demo".into(),
        parent: None,
        spawned_ms: None,
        ended_ms: None,
        sessions: if session.is_empty() {
            vec![]
        } else {
            vec![session.into()]
        },
    };
    let pair = |from: &str, to: &str, messages: u64| MessageCount {
        from: from.into(),
        to: to.into(),
        messages,
    };
    let corpus = Corpus {
        sessions: vec![session("a", "sa"), session("b", "sb")],
        calls: number(vec![
            call("a", T0 + HOUR, None, 1, tokens(0, 30_000, 0, 1)),
            call("b", T0 + HOUR, None, 1, tokens(0, 30_000, 0, 1)),
        ]),
        turns: vec![
            turn("a", 1, "peer", Some("pij-boss")),
            turn("b", 1, "human", None),
        ],
        seats: vec![
            seat("pij-worker-a", "sa"),
            seat("pij-worker-b", "sb"),
            seat("pij-boss", ""),
        ],
        messages: vec![
            pair("pij-boss", "pij-worker-a", 5),
            pair("pij-boss", "pij-worker-b", 3),
            pair("pij-worker-a", "pij-boss", 2),
            pair("pij-far-otter", "pij-worker-a", 1),
            pair("pij-x", "pij-y", 9),
        ],
        primes: vec!["pij-boss".into()],
        prime_projects: [("pij-boss".to_string(), vec!["demo".to_string()])].into(),
        ..Corpus::default()
    };
    let report = analyze(&corpus, window(), &PriceTable::default());
    let g = &report.graph;
    assert_eq!(g.messages.len(), 4, "{:?}", g.messages);
    assert!(g.messages.iter().all(|m| m.from != "pij-x"));
    let node = |id: &str| {
        g.nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("{id}"))
    };
    assert!(node("pij-boss").prime && node("pij-boss").local);
    assert_eq!(
        node("pij-boss").folder.as_deref(),
        Some("/work/demo"),
        "where it works"
    );
    assert_eq!(node("pij-boss").projects, ["demo"], "what it is prime for");
    assert!(!node("pij-far-otter").local, "another machine's seat");
    assert!(!node("pij-worker-a").prime);
    assert_eq!(
        (node("pij-worker-a").sent, node("pij-worker-a").received),
        (2, 6)
    );
    assert!(g.nodes.iter().all(|n| n.id != "pij-x"));
}

/// A typed turn is the user's, whoever the user is: no operator name in a report.
#[test]
fn a_typed_turn_is_labelled_user_typed() {
    assert_eq!(super::analysis::group("human"), "user typed");
    assert!(super::analysis::GROUPS.contains(&"user typed"));
}
