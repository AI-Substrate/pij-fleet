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
            "add",
            "notes",
            "--type",
            "worker",
            "--title",
            "Note taker",
            "--holders",
            "many",
            "--param",
            "subject",
            "--param",
            "session:optional",
            "--rule",
            "One note per <subject> call.",
            "--packet",
            "Publish to <session>.",
            "--why",
            "w",
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
    assert_eq!(
        missing["details"]["code"], "E-RS-PARAM-MISSING",
        "{missing}"
    );
    assert!(
        missing["meta"]
            .as_str()
            .unwrap()
            .contains("--set subject=<value>")
    );
    let unknown = directory(
        addr,
        &repo,
        &[
            "show",
            "notes",
            "--first-turn",
            "--set",
            "subject=a",
            "--set",
            "topic=b",
        ],
    )
    .await;
    assert_eq!(
        unknown["details"]["code"], "E-RS-PARAM-UNKNOWN",
        "{unknown}"
    );
    assert!(
        unknown["meta"]
            .as_str()
            .unwrap()
            .contains("subject, session")
    );
    let bare = directory(addr, &repo, &["show", "notes"]).await;
    assert_eq!(bare["details"]["code"], "E-RS-ARG", "{bare}");

    std::fs::write(repo.main.join("packet.md"), "Bring last week's actions.\n").unwrap();
    let shown = directory(
        addr,
        &repo,
        &[
            "show",
            "notes",
            "--first-turn",
            "--set",
            "subject=planning",
            "--focus",
            "Start with the agenda.",
            "--packet",
            "packet.md",
            "--instance",
            "s1",
        ],
    )
    .await;
    assert_eq!(shown["ok"], true, "{shown}");
    let text = shown["data"]["first_turn"].as_str().unwrap();
    assert!(text.starts_with("# First turn: role notes/s1\n"), "{text}");
    assert!(text.contains("One note per planning call."), "{text}");
    assert!(
        text.contains("Publish to <session>."),
        "an unset optional stays: {text}"
    );
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

const FIRST_TURNS: &str = ".harness/government/community/roles/notes/first-turns";

async fn add_notes(addr: SocketAddr, repo: &Repo) {
    let added = directory(
        addr,
        repo,
        &[
            "add",
            "notes",
            "--type",
            "worker",
            "--title",
            "Note taker",
            "--holders",
            "many",
            "--param",
            "subject",
            "--rule",
            "One note per <subject> call.",
            "--packet",
            "p",
            "--why",
            "w",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
}

async fn spawn(addr: SocketAddr, repo: &Repo, as_role: Option<Value>) -> Value {
    let mut body = json!({
        "harness": "omp",
        "cwd": repo.main,
        "session": "work",
        "no_wait": true,
        "caller": {"TMUX_PANE": PANE, "cwd": repo.main},
    });
    if let Some(as_role) = as_role {
        body["as_role"] = as_role;
    }
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth(KEY)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn launches(tmux: &FakeTmux) -> usize {
    tmux.calls()
        .iter()
        .filter(|call| call.starts_with("new_window:"))
        .count()
}

/// Plan 242 ac-0002 to ac-0006: a spawn with a role launches as a plain
/// spawn does, then ONE writer commit records its first turn and holding,
/// then the first turn is queued as the seat's first message. A refusal
/// before the launch launches nothing; a spawn with no role warns; a missing
/// governance seed warns until `seed --shipped` copies it.
#[tokio::test]
async fn spawn_as_a_role_is_a_plain_spawn_plus_one_commit_and_its_first_turn() {
    let repo = repo("pij-spawn-as");
    let rig = rig(Arc::new(NoHook)).await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    add_notes(addr, &repo).await;

    let plain = spawn(addr, &repo, None).await;
    assert_eq!(plain["ok"], true, "{plain}");
    assert!(
        plain["details"]["warning"]
            .as_str()
            .unwrap()
            .contains("starts in no role"),
        "{plain}"
    );
    assert_eq!(launches(&rig.tmux), 1);

    let unfilled = spawn(addr, &repo, Some(json!({"role": "notes"}))).await;
    assert_eq!(
        unfilled["details"]["code"], "E-RS-PARAM-MISSING",
        "{unfilled}"
    );
    assert_eq!(unfilled["details"]["step"], "building the first turn");
    let missing = spawn(addr, &repo, Some(json!({"role": "nobody"}))).await;
    assert_eq!(missing["ok"], false, "{missing}");
    assert_eq!(
        launches(&rig.tmux),
        1,
        "a refusal before the launch launches nothing"
    );

    let before = head(&repo);
    let started = spawn(
        addr,
        &repo,
        Some(json!({"role": "notes", "instance": "s1", "set": ["subject=planning"], "focus": "Start with the agenda."})),
    )
    .await;
    assert_eq!(started["ok"], true, "{started}");
    assert_eq!(launches(&rig.tmux), 2, "one launch, as a plain spawn");
    let seat = started["data"]["id"].as_str().unwrap().to_string();
    let count = git(
        &repo.gov,
        &["rev-list", "--count", &format!("{before}..HEAD")],
    );
    assert_eq!(count.trim(), "1", "one writer commit: {started}");
    let role = &started["details"]["role"];
    assert_eq!(role["commit"], head(&repo));
    let recorded =
        std::fs::read_to_string(repo.gov.join(format!("{FIRST_TURNS}/{seat}.md"))).unwrap();
    assert!(
        recorded.starts_with("# First turn: role notes/s1\n"),
        "{recorded}"
    );
    assert!(
        recorded.contains("One note per planning call."),
        "{recorded}"
    );
    assert!(recorded.contains("Start with the agenda."), "{recorded}");
    let held = rig.store.open_assignments(&seat).await.unwrap();
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(
        (held[0].role.as_str(), held[0].instance.as_str()),
        ("notes", "s1")
    );
    assert_eq!(held[0].assigned_by, SEAT);
    let inbox = rig
        .delivery
        .peek_inbox(&SeatId::from(seat.as_str()))
        .await
        .unwrap();
    assert_eq!(
        inbox[0].message.body, recorded,
        "the first turn is its first message"
    );
    assert_eq!(inbox[0].message.from.as_str(), SEAT, "from its creator");
    let warnings = started["details"]["warnings"].as_array().unwrap();
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains("pij directory seed worker --shipped"),
        "{started}"
    );

    let seeded = directory(addr, &repo, &["seed", "worker", "--shipped", "--why", "w"]).await;
    assert_eq!(seeded["ok"], true, "{seeded}");
    let again = spawn(
        addr,
        &repo,
        Some(json!({"role": "notes", "instance": "s2", "set": ["subject=retro"]})),
    )
    .await;
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(again["details"]["warnings"], json!([]), "{again}");
    rig.server.abort();
}

/// Refuses the holders document while `refuse` is set; else as [`FakeDdocs`].
struct RefusesHolders(Arc<std::sync::atomic::AtomicBool>);

impl Validator for RefusesHolders {
    fn validate(&self, top: &Path, document: &Path) -> Result<(), Refusal> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) && document.ends_with("holders.dd.json")
        {
            return Err(Refusal {
                code: pij_daemon::community::E_DDOCS_INVALID,
                message: "injected refusal".into(),
                next: None,
            });
        }
        FakeDdocs.validate(top, document)
    }

    fn build(&self, top: &Path, document: &Path) -> Result<(), Refusal> {
        FakeDdocs.build(top, document)
    }
}

/// Plan 242 ac-0003: a refused role commit rolls the spawn back through the
/// failed-row-write path: its own pane is removed, its row tombstoned, it
/// holds nothing and was sent nothing, and the refusal names the step.
#[tokio::test]
async fn a_refused_role_commit_rolls_the_spawn_back() {
    let repo = repo("pij-spawn-as-refused");
    let refuse = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = CommunityWriter::new(
        Arc::new(RefusesHolders(Arc::clone(&refuse))),
        vec![],
        &LOCK_RETRY,
    );
    let rig = rig_with(Arc::new(NoHook), writer).await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    add_notes(addr, &repo).await;
    let before = head(&repo);
    refuse.store(true, std::sync::atomic::Ordering::SeqCst);

    let refused = spawn(
        addr,
        &repo,
        Some(json!({"role": "notes", "set": ["subject=planning"]})),
    )
    .await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        refused["details"]["step"], "the one writer commit (first turn and holding)",
        "{refused}"
    );
    assert_eq!(head(&repo), before, "nothing committed");
    assert_eq!(launches(&rig.tmux), 1);
    let seats = rig
        .services
        .registry
        .list(pij_core::ports::SeatFilter {
            folder: Some(repo.main.display().to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    let [seat] = seats.as_slice() else {
        panic!("one launched seat: {seats:?}");
    };
    assert!(
        seat.tombstoned_at.is_some(),
        "its row is tombstoned: {seat:?}"
    );
    let pane = seat.pane.clone().unwrap();
    assert!(
        rig.tmux.calls().contains(&format!("kill:{pane}")),
        "its own pane is removed: {:?}",
        rig.tmux.calls()
    );
    assert!(
        rig.store
            .open_assignments(seat.id.as_str())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        rig.delivery.peek_inbox(&seat.id).await.unwrap().is_empty(),
        "nothing delivered"
    );
    rig.server.abort();
}
