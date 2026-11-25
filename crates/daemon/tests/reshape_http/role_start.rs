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

pub(super) async fn add_notes(addr: SocketAddr, repo: &Repo) {
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

pub(super) async fn spawn(addr: SocketAddr, repo: &Repo, as_role: Option<Value>) -> Value {
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

pub(super) fn launches(tmux: &FakeTmux) -> usize {
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

/// Plan 243: a live seat working in the project with no role there is a
/// `no-role` action with the assign and the close; after the assign it is gone.
#[tokio::test]
async fn a_seat_working_here_with_no_role_is_an_action_until_assigned() {
    let repo = repo("pij-no-role");
    let rig = rig(Arc::new(NoHook)).await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    add_notes(addr, &repo).await;
    let plain = spawn(addr, &repo, None).await;
    let seat = plain["data"]["id"].as_str().unwrap().to_string();
    let no_role = |view: &Value| -> Vec<Value> {
        view["data"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| action["condition"] == "no-role")
            .cloned()
            .collect()
    };

    let view = directory(addr, &repo, &[]).await;
    let actions = no_role(&view);
    assert_eq!(actions.len(), 1, "only the seat in this repo: {view}");
    assert_eq!(actions[0]["seat"], seat.as_str());
    let commands = actions[0]["commands"].as_array().unwrap();
    assert!(
        commands[0]
            .as_str()
            .unwrap()
            .starts_with("pij directory assign '<role>' ")
    );
    assert_eq!(commands[1], format!("pij close {seat}"));

    let assigned = directory(addr, &repo, &["assign", "notes/s9", &seat, "--why", "w"]).await;
    assert_eq!(assigned["ok"], true, "{assigned}");
    assert!(no_role(&directory(addr, &repo, &[]).await).is_empty());
    rig.server.abort();
}

/// Plan 244 ac-0001 to ac-0003: a seat that is not the prime adds a role and
/// starts a seat in it; the prime gets one FYI for each, naming the role, its
/// type, the creator and the reason, and the view shows who created and who
/// assigned. A suggestion is recorded, listed and answered, waking no one.
#[tokio::test]
async fn the_prime_is_told_of_what_others_create_and_suggestions_are_answered() {
    let repo = repo("pij-creator-note");
    let rig = rig(Arc::new(NoHook)).await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    let prime = "pij-holder";
    let set = call(
        addr,
        "/v1/project",
        &["project", "set", "fleet", "--prime", prime],
        &repo.main,
    )
    .await;
    assert_eq!(set["ok"], true, "{set}");
    add_notes(addr, &repo).await;
    let started = spawn(
        addr,
        &repo,
        Some(json!({"role": "notes", "instance": "s1", "set": ["subject=planning"], "focus": "the planning call"})),
    )
    .await;
    assert_eq!(started["details"]["prime_told"]["sent"], "fyi", "{started}");
    let seat = started["data"]["id"].as_str().unwrap().to_string();
    let (fyis, _) = rig
        .delivery
        .claim_fyis(&SeatId::from(prime), "test")
        .await
        .unwrap();
    // The add's routes-change notice (plan 202) is held too; these are the two notes.
    let bodies: Vec<&str> = fyis
        .iter()
        .map(|fyi| fyi.body.as_str())
        .filter(|body| body.starts_with("pij: "))
        .collect();
    assert_eq!(bodies.len(), 2, "{bodies:?}");
    assert!(
        bodies[0].contains(&format!("{SEAT} added role notes (worker)"))
            && bodies[0].contains("\"w\""),
        "{bodies:?}"
    );
    assert!(
        bodies[1].contains(&format!("{SEAT} started {seat} in role notes/s1 (worker)"))
            && bodies[1].contains("the planning call"),
        "{bodies:?}"
    );

    let view = directory(addr, &repo, &[]).await;
    let notes = view["data"]["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|role| role["id"] == "notes")
        .unwrap()
        .clone();
    assert_eq!(notes["created_by"], SEAT, "{notes}");
    let holder = notes["live"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["seat"] == seat.as_str()))
        .cloned();
    assert_eq!(
        holder.map(|row| row["assigned_by"].clone()),
        Some(json!(SEAT)),
        "{notes}"
    );

    let raised = directory(
        addr,
        &repo,
        &["suggest", "docs/", "--signal", "nobody keeps the docs"],
    )
    .await;
    assert_eq!(raised["ok"], true, "{raised}");
    assert_eq!(raised["data"]["suggestion"]["id"], "sg-0001");
    assert_eq!(
        rig.delivery
            .pending_fyi_count(&SeatId::from(prime))
            .await
            .unwrap(),
        0,
        "a suggestion wakes no one and holds nothing"
    );
    let open = directory(addr, &repo, &[]).await;
    assert_eq!(open["data"]["seat_suggestions"][0]["area"], "docs/");
    let answered = directory(
        addr,
        &repo,
        &[
            "suggest",
            "--answer",
            "sg-0001",
            "not-yet",
            "--why",
            "after the release",
        ],
    )
    .await;
    assert_eq!(answered["ok"], true, "{answered}");
    let later = directory(addr, &repo, &[]).await;
    assert_eq!(
        later["data"]["seat_suggestions"][0]["answer"]["why"],
        "after the release"
    );
    let bad = directory(
        addr,
        &repo,
        &["suggest", "--answer", "sg-0001", "maybe", "--why", "w"],
    )
    .await;
    assert_eq!(bad["details"]["code"], "E-RS-ARG", "{bad}");
    rig.server.abort();
}

/// Review of #345, N1: a role commit that lands but cannot be indexed keeps
/// the seat (the holding and first turn are on the branch) and warns with
/// the fix; only a commit that never landed rolls the spawn back.
#[tokio::test]
async fn a_role_commit_that_lands_but_is_not_indexed_keeps_the_seat_and_warns() {
    let repo = repo("pij-spawn-as-unindexed");
    let rig = rig(Arc::new(NoHook)).await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    add_notes(addr, &repo).await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig._fresh.path()))
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER refuse_index BEFORE INSERT ON community_assignments \
         BEGIN SELECT RAISE(ABORT, 'injected index failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let before = head(&repo);

    let started = spawn(
        addr,
        &repo,
        Some(json!({"role": "notes", "set": ["subject=planning"]})),
    )
    .await;
    assert_eq!(started["ok"], true, "{started}");
    let seat = started["data"]["id"].as_str().unwrap().to_string();
    let count = git(
        &repo.gov,
        &["rev-list", "--count", &format!("{before}..HEAD")],
    );
    assert_eq!(count.trim(), "1", "the commit landed");
    let warning = started["details"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find(|warning| warning.contains("could not index"))
        .unwrap_or_else(|| panic!("an indexing warning: {started}"))
        .to_string();
    assert!(
        warning.contains("pij directory rebuild --project fleet"),
        "{warning}"
    );
    let row = rig
        .services
        .registry
        .get(&SeatId::from(seat.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert!(row.tombstoned_at.is_none(), "the seat is kept: {row:?}");
    assert!(
        !rig.tmux
            .calls()
            .iter()
            .any(|call| call.starts_with("kill:")),
        "no pane is removed"
    );
    rig.server.abort();
}

/// Review of #345, N5: a `--packet` outside the caller's folder and the repo
/// is refused, and creator text that looks like a secret is refused naming
/// where, never the text.
#[tokio::test]
async fn a_first_turn_reads_no_outside_packet_and_commits_no_secret() {
    let repo = repo("pij-first-turn-brakes");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    add_notes(addr, &repo).await;
    let outside = repo.root.join("outside.md");
    std::fs::write(&outside, "elsewhere\n").unwrap();
    let refused = directory(
        addr,
        &repo,
        &[
            "show",
            "notes",
            "--first-turn",
            "--set",
            "subject=a",
            "--packet",
            outside.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(
        refused["details"]["code"], "E-RS-PACKET-OUTSIDE",
        "{refused}"
    );
    let secret = directory(
        addr,
        &repo,
        &[
            "show",
            "notes",
            "--first-turn",
            "--set",
            "subject=a",
            "--focus",
            "api_key = abcdefgh1234",
        ],
    )
    .await;
    assert_eq!(
        secret["details"]["code"], "E-RS-FIRST-TURN-SECRET",
        "{secret}"
    );
    assert!(!secret.to_string().contains("abcdefgh1234"), "{secret}");
    server.abort();
}
