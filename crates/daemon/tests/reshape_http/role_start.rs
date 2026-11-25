//! PRD 007 (plans 240, 241): a role's parameters and the first turn a seat
//! starting in it gets, through `/v1/directory` against scratch governance
//! repos.

use super::*;

/// Plan 240 ac-0001, plan 241 ac-0002: a role declares its parameters; `show
/// --first-turn` fills them, refuses as a spawn would, and writes nothing.
#[tokio::test]
async fn show_first_turn_fills_parameters_and_refuses_as_a_spawn_would() {
    let repo = repo("pij-first-turn");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    let added = directory(
        addr,
        &repo,
        &[
            "add", "notes", "--type", "worker", "--title", "Note taker", "--holders", "many",
            "--param", "subject", "--param", "session:optional", "--rule",
            "One note per <subject> call.", "--packet", "Publish to <session>.", "--why", "w",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let parameters = section(&committed(&repo, "notes"), "parameters");
    assert_eq!(
        parameters,
        json!([{"name": "subject", "need": "required"}, {"name": "session", "need": "optional"}])
    );
    let start = head(&repo);

    let missing = directory(addr, &repo, &["show", "notes", "--first-turn"]).await;
    assert_eq!(missing["details"]["code"], "E-RS-PARAM-MISSING", "{missing}");
    assert!(missing["meta"].as_str().unwrap().contains("--set subject=<value>"));
    let unknown = directory(
        addr,
        &repo,
        &["show", "notes", "--first-turn", "--set", "subject=a", "--set", "topic=b"],
    )
    .await;
    assert_eq!(unknown["details"]["code"], "E-RS-PARAM-UNKNOWN", "{unknown}");
    assert!(unknown["meta"].as_str().unwrap().contains("subject, session"));
    let bare = directory(addr, &repo, &["show", "notes"]).await;
    assert_eq!(bare["details"]["code"], "E-RS-ARG", "{bare}");

    std::fs::write(repo.main.join("packet.md"), "Bring last week's actions.\n").unwrap();
    let shown = directory(
        addr,
        &repo,
        &[
            "show", "notes", "--first-turn", "--set", "subject=planning", "--focus",
            "Start with the agenda.", "--packet", "packet.md", "--instance", "s1",
        ],
    )
    .await;
    assert_eq!(shown["ok"], true, "{shown}");
    let text = shown["data"]["first_turn"].as_str().unwrap();
    assert!(text.starts_with("# First turn: role notes/s1\n"), "{text}");
    assert!(text.contains("One note per planning call."), "{text}");
    assert!(text.contains("Publish to <session>."), "an unset optional stays: {text}");
    assert!(text.contains("Start with the agenda."), "{text}");
    assert!(text.contains("Bring last week's actions."), "{text}");
    let headings: Vec<&str> = shown["data"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| part["heading"].as_str().unwrap())
        .collect();
    assert_eq!(headings[0], "Your seat type: worker", "{headings:?}");
    assert_eq!(headings[1], "Your role: notes/s1", "{headings:?}");
    assert_eq!(headings[2], "Parameters", "{headings:?}");
    assert!(headings[3].starts_with("Focus from "), "{headings:?}");
    assert!(headings[4].starts_with("Packet from "), "{headings:?}");
    assert_eq!(head(&repo), start, "show writes nothing");
    server.abort();
}
