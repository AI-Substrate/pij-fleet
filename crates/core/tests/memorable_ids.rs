use std::collections::{HashMap, HashSet};

use pij_core::model::SeatId;
use pij_core::names::corpus::{EXCLUDED_NAME_WORDS, NAME_ADJECTIVES, NAME_NOUNS, SHIP_NAMES};
use pij_core::names::{
    MEMORABLE_PIJ_ID_SPACE, MemorableIdError, memorable_pij_id_candidate,
    memorable_pij_id_candidates,
};

fn candidate(seed: &str, attempt: usize) -> SeatId {
    memorable_pij_id_candidate(seed, attempt).expect("candidate inside pinned space")
}

#[test]
fn curated_corpus_is_pinned_and_clean() {
    // Plan 176 swept the corpus; every ship name went, so the space is the grid.
    assert_eq!(NAME_ADJECTIVES.len(), 722);
    assert_eq!(NAME_NOUNS.len(), 266);
    assert!(SHIP_NAMES.is_empty());
    assert_eq!(MEMORABLE_PIJ_ID_SPACE, 192_052);
    assert_eq!(
        MEMORABLE_PIJ_ID_SPACE,
        NAME_ADJECTIVES.len() * NAME_NOUNS.len() + SHIP_NAMES.len()
    );

    assert_eq!(
        NAME_ADJECTIVES
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len(),
        NAME_ADJECTIVES.len()
    );
    assert_eq!(
        NAME_NOUNS.iter().copied().collect::<HashSet<_>>().len(),
        NAME_NOUNS.len()
    );
    assert_eq!(
        SHIP_NAMES.iter().copied().collect::<HashSet<_>>().len(),
        SHIP_NAMES.len()
    );

    for word in NAME_ADJECTIVES.iter().chain(NAME_NOUNS.iter()) {
        assert!(
            !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()),
            "invalid corpus word: {word}"
        );
    }
    for ship in SHIP_NAMES {
        assert!(
            ship.len() <= 26
                && ship
                    .split('-')
                    .all(|word| !word.is_empty()
                        && word.bytes().all(|byte| byte.is_ascii_lowercase())),
            "invalid ship name: {ship}"
        );
    }
}

#[test]
fn excluded_words_never_reach_a_seat_id() {
    // The reported case (plan 176: a seat named pij-gay-thorn) stays pinned.
    for reported in ["gay", "thorn"] {
        assert!(
            EXCLUDED_NAME_WORDS.contains(&reported),
            "{reported} left the exclusion list"
        );
    }

    // A ship name is emitted whole, so each of its words is checked too.
    let ship_words = SHIP_NAMES
        .iter()
        .flat_map(|ship| ship.split('-').map(move |word| (*ship, word)));
    let mut returned = Vec::new();
    for excluded in EXCLUDED_NAME_WORDS {
        if NAME_ADJECTIVES.contains(excluded) {
            returned.push(format!("adjective {excluded}"));
        }
        if NAME_NOUNS.contains(excluded) {
            returned.push(format!("noun {excluded}"));
        }
        if SHIP_NAMES.contains(excluded) {
            returned.push(format!("ship {excluded}"));
        }
    }
    for (ship, word) in ship_words {
        if EXCLUDED_NAME_WORDS.contains(&word) {
            returned.push(format!("ship {ship} (word {word})"));
        }
    }
    assert!(returned.is_empty(), "excluded words returned: {returned:?}");
}

#[test]
fn rust_matches_the_typescript_oracle() {
    // Oracle: the TypeScript production implementation in this checkout.
    // Regenerate after any corpus change; Bun runs it without node_modules.
    // The shell variable preserves NUL escapes until Bun parses the literals:
    //
    // ORACLE_SCRIPT='import { memorablePijIdCandidate, MEMORABLE_PIJ_ID_SPACE } from
    // "./.omp/extensions/pij/core/memorable-id.ts"; const cases =
    // [["4dffbee3-5d42-4b02-a739-74be162940bc",0],["copilot-session-alpha",0],
    // ["claude-session-beta",0],["pi-session-gamma",0],["s1754000000000-9000",0],
    // ["s1754000000000-9000",1],["s1754000000000-9000",17],
    // ["adopt\x00claude\x00%100\x0020000",0],["adopt\x00claude\x00%100\x0020000",1],
    // ["adopt\x00claude\x00%100\x0020000",17],["full-space-seed",0],
    // ["full-space-seed",1],["full-space-seed",192012],["full-space-seed",192051],
    // ["stable-seed",192051],["café-🐙",0],["café-🐙",1]];
    // console.log(JSON.stringify({space:MEMORABLE_PIJ_ID_SPACE,vectors:cases.map(([seed,
    // attempt])=>({seed,attempt,result:memorablePijIdCandidate(seed,attempt)}))},null,2));'
    // bun --eval "$ORACLE_SCRIPT"
    const GOLDENS: &[(&str, usize, &str)] = &[
        (
            "4dffbee3-5d42-4b02-a739-74be162940bc",
            0,
            "pij-preferred-caribou",
        ),
        ("copilot-session-alpha", 0, "pij-formal-boar"),
        ("claude-session-beta", 0, "pij-extensive-capybara"),
        ("pi-session-gamma", 0, "pij-exact-armadillo"),
        ("s1754000000000-9000", 0, "pij-symbolic-aphid"),
        ("s1754000000000-9000", 1, "pij-living-mole"),
        ("s1754000000000-9000", 17, "pij-planned-badger"),
        ("adopt\x00claude\x00%100\x0020000", 0, "pij-casual-earwig"),
        ("adopt\x00claude\x00%100\x0020000", 1, "pij-relaxed-shark"),
        (
            "adopt\x00claude\x00%100\x0020000",
            17,
            "pij-surprised-egret",
        ),
        ("full-space-seed", 0, "pij-rapid-moose"),
        ("full-space-seed", 1, "pij-formal-buzzard"),
        ("full-space-seed", 192_012, "pij-funny-hare"),
        ("full-space-seed", 192_051, "pij-blank-badger"),
        ("stable-seed", 192_051, "pij-intelligent-earthworm"),
        ("café-🐙", 0, "pij-complicated-walrus"),
        ("café-🐙", 1, "pij-significant-lark"),
    ];

    for &(seed, attempt, expected) in GOLDENS {
        assert_eq!(
            candidate(seed, attempt).as_str(),
            expected,
            "seed={seed:?} attempt={attempt}"
        );
    }
}

#[test]
fn candidate_sequence_is_complete_without_repeats() {
    let ship_ids = SHIP_NAMES
        .iter()
        .map(|ship| SeatId::from(format!("pij-{ship}")))
        .collect::<HashSet<_>>();

    for seed in ["stable-seed", "full-space-seed"] {
        let candidates = memorable_pij_id_candidates(seed);
        assert_eq!(candidates.len(), MEMORABLE_PIJ_ID_SPACE);
        let ids = candidates.collect::<HashSet<_>>();
        assert_eq!(ids.len(), MEMORABLE_PIJ_ID_SPACE, "seed={seed}");
        assert!(ship_ids.is_subset(&ids), "seed={seed}");
    }

    assert_eq!(candidate("stable-seed", 0).as_str(), "pij-calm-seahorse");
    assert!(!ship_ids.contains(&candidate("full-space-seed", 0)));
    assert_eq!(
        memorable_pij_id_candidate("seed", MEMORABLE_PIJ_ID_SPACE),
        Err(MemorableIdError::Exhausted {
            space: MEMORABLE_PIJ_ID_SPACE
        })
    );
}

#[test]
fn collision_probes_leave_the_starting_adjective_row() {
    // Treat the first 80 candidates as 79 occupied slots. A stride of one can
    // cross at most one 266-noun row boundary; the selected coprime stride must
    // spread the collision sequence across the grid instead.
    let adjectives = memorable_pij_id_candidates("stable-seed")
        .take(80)
        .map(|id| {
            id.as_str()
                .strip_prefix("pij-")
                .and_then(|rest| rest.split('-').next())
                .expect("stable-seed opens in the pair grid")
                .to_string()
        })
        .collect::<HashSet<_>>();

    assert!(
        adjectives.len() > 60,
        "80 collision probes reached only {} adjective rows",
        adjectives.len()
    );
}

fn spawn_seed(index: usize) -> String {
    format!(
        "s{}-{}",
        1_754_000_000_000_u64 + index as u64 * 91_733,
        9_000 + index
    )
}

fn adopt_seed(index: usize) -> String {
    format!(
        "adopt\0claude\0%{}\0{}",
        100 + index * 3,
        20_000 + index * 7
    )
}

fn uuid_seed(index: usize) -> String {
    format!("4dffbee3-5d42-4b02-a739-74be1629{index:04}")
}

type SeedFactory = fn(usize) -> String;

#[test]
fn realistic_collision_allocation_does_not_funnel_into_adjectives() {
    let ship_ids = SHIP_NAMES
        .iter()
        .map(|ship| format!("pij-{ship}"))
        .collect::<HashSet<_>>();
    let shapes: &[(&str, SeedFactory)] = &[
        ("spawn", spawn_seed),
        ("adopt", adopt_seed),
        ("uuid", uuid_seed),
    ];

    for &(shape, seed_for) in shapes {
        let mut taken = HashSet::new();
        let mut per_adjective = HashMap::<String, usize>::new();
        for index in 0..405 {
            let id = memorable_pij_id_candidates(&seed_for(index))
                .find(|candidate| taken.insert(candidate.clone()))
                .expect("405 allocations fit in the pinned name space");
            if !ship_ids.contains(id.as_str()) {
                let adjective = id
                    .as_str()
                    .strip_prefix("pij-")
                    .and_then(|rest| rest.split('-').next())
                    .expect("pair candidate shape");
                *per_adjective.entry(adjective.to_string()).or_default() += 1;
            }
        }

        assert_eq!(taken.len(), 405, "shape={shape}");
        let worst = per_adjective.values().copied().max().unwrap_or_default();
        assert!(worst < 8, "shape={shape} worst adjective count={worst}");
        assert!(
            per_adjective.len() > 250,
            "shape={shape} adjective count={}",
            per_adjective.len()
        );
    }
}

#[test]
fn no_seed_opens_on_a_ship_name() {
    // Plan 176 emptied SHIP_NAMES, so every candidate is pij-<adjective>-<noun>.
    assert!(SHIP_NAMES.is_empty());
    for index in 0..3_000 {
        let id = candidate(&spawn_seed(index), 0);
        let words = id
            .as_str()
            .strip_prefix("pij-")
            .and_then(|rest| rest.split_once('-'))
            .expect("pair candidate shape");
        assert!(
            NAME_ADJECTIVES.contains(&words.0) && NAME_NOUNS.contains(&words.1),
            "not a corpus pair: {id}"
        );
    }
}
