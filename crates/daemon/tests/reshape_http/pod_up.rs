//! PRD 008 s02 (plan 248): `pij pod plan`, `up`, `show` and the tool canary,
//! through `/v1/pod` against scratch governance repos.

use super::*;

const POD: &str = ".harness/government/community/pods/build/pod.dd.json";

async fn pod(addr: SocketAddr, repo: &Repo, argv: &[&str]) -> Value {
    let mut full = vec!["pod"];
    full.extend_from_slice(argv);
    call(addr, "/v1/pod", &full, &repo.main).await
}

/// A template at `pods/build` in the main checkout: a lead naming tools, a
/// reviewer that must not share the lead's model, a consulted role needed
/// before up, and a deliverable owned by a role the project lacks.
fn template(repo: &Repo) {
    let folder = repo.main.join("pods/build");
    std::fs::create_dir_all(&folder).unwrap();
    let document = json!({
        "dd": {"schema": "pij/pod-template"},
        "sections": [
            {"name": "meta", "value": {"name": "build", "title": "Build pod", "version": "1"}},
            {"name": "parameters", "value": [
                {"name": "subject", "need": "required"},
                {"name": "work-folder", "need": "optional"},
                {"name": "branch", "need": "optional"},
            ]},
            {"name": "slots", "value": [
                {"id": "lead", "role": "notes", "instance": "lead", "parameters": ["subject"], "tools": ["git", "gh"]},
                {"id": "reviewer", "role": "notes", "instance": "review", "parameters": ["subject", "branch"], "not_model_of": "lead", "keep_open": "yes"},
            ]},
            {"name": "consulted", "value": [{"role": "security", "before_up": "yes"}]},
            {"name": "deliverables", "value": [{"id": "report", "owner": "keeper"}]},
        ],
        "references": [],
    });
    std::fs::write(
        folder.join("pod-template.dd.json"),
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();
}

fn abouts(answer: &Value) -> Vec<String> {
    answer["data"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["about"].as_str().unwrap().to_string())
        .collect()
}

/// Plan 248 ac-0001: plan prints every warning with its fix, and writes and
/// launches nothing.
#[tokio::test]
async fn a_pod_plan_names_every_warning_with_its_fix_and_writes_nothing() {
    let repo = repo("pij-pod-plan");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let before = head(&repo);

    let unfilled = pod(rig.addr, &repo, &["plan", "pods/build"]).await;
    assert_eq!(unfilled["details"]["code"], "E-RS-POD-PARAM", "{unfilled}");
    let planned = pod(
        rig.addr,
        &repo,
        &["plan", "pods/build", "--set", "subject=planning"],
    )
    .await;
    assert_eq!(planned["ok"], true, "{planned}");
    assert_eq!(
        abouts(&planned),
        [
            "consulted role security",
            "slot reviewer",
            "deliverable report"
        ],
        "{planned}"
    );
    let lines: Vec<&str> = planned["data"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["line"].as_str().unwrap())
        .collect();
    assert!(
        lines[0].contains("Fix: pij spawn --as role:security"),
        "{lines:?}"
    );
    assert!(
        lines[2].contains("Fix: pij directory add keeper"),
        "{lines:?}"
    );
    let slots = planned["data"]["slots"].as_array().unwrap();
    assert!(
        slots[0]["first_turn"]
            .as_str()
            .unwrap()
            .contains("One note per planning call."),
        "{planned}"
    );
    assert_eq!(slots[0]["tools"], json!(["git", "gh"]));
    assert_eq!(head(&repo), before, "plan writes nothing");
    assert_eq!(
        super::role_start::launches(&rig.tmux),
        0,
        "plan launches nothing"
    );
    rig.server.abort();
}

/// Plan 248 ac-0002, ac-0004: up records the pod, starts each slot once as
/// `spawn --as` does, records its members and logs every warning; it never
/// refuses on one. A second up of the same pod is refused by name.
#[tokio::test]
async fn pod_up_records_starts_each_slot_once_and_logs_its_warnings() {
    let repo = repo("pij-pod-up");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let before = head(&repo);

    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--session",
            "work",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    assert_eq!(
        super::role_start::launches(&rig.tmux),
        2,
        "one spawn per slot: {up}"
    );
    let count = git(
        &repo.gov,
        &["rev-list", "--count", &format!("{before}..HEAD")],
    );
    assert_eq!(
        count.trim(),
        "4",
        "the record, one commit per slot, the members: {up}"
    );
    let members = up["data"]["members"].as_array().unwrap();
    assert_eq!(members.len(), 2, "{up}");
    assert_eq!(members[0]["canary"], "pending");
    assert_eq!(members[1]["canary"], "none asked");
    let record: Value =
        serde_json::from_slice(&std::fs::read(repo.gov.join(POD)).unwrap()).unwrap();
    assert_eq!(section(&record, "meta")["template"], "pods/build");
    let log = section(&record, "log").to_string();
    assert!(log.contains("warning: consulted role security"), "{log}");
    assert!(log.contains("warning: deliverable report"), "{log}");

    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(shown["ok"], true, "{shown}");
    assert_eq!(shown["data"]["members"][0]["seat"], members[0]["seat"]);
    assert_eq!(shown["data"]["consulted"][0]["live_holders"], json!([]));
    assert_eq!(shown["data"]["deliverables"][0]["owner"], "keeper");

    let again = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--session",
            "work",
        ],
    )
    .await;
    assert_eq!(again["details"]["code"], "E-RS-POD-EXISTS", "{again}");
    assert_eq!(super::role_start::launches(&rig.tmux), 2);
    rig.server.abort();
}

/// Plan 248 ac-0003: the seat in a slot naming tools is asked to confirm
/// them; one it leaves out degrades the slot, shown with its fix, and the
/// seat that ran up is told. Nothing is stopped or refused.
#[tokio::test]
async fn a_tool_a_seat_does_not_confirm_degrades_its_slot_with_the_fix() {
    let repo = repo("pij-pod-canary");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--session",
            "work",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let lead = up["data"]["members"][0]["seat"]
        .as_str()
        .unwrap()
        .to_string();

    let confirmed: Value = reqwest::Client::new()
        .post(format!("http://{}/v1/pod", rig.addr))
        .bearer_auth(KEY)
        .json(&json!({
            "argv": ["pod", "confirm", "build", "--tool", "git"],
            "caller": {"PIJ_SESSION_ID": lead, "cwd": repo.main},
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(confirmed["ok"], true, "{confirmed}");
    let deadline = std::time::Instant::now() + scaled(std::time::Duration::from_secs(10));
    let canary = loop {
        let record: Value =
            serde_json::from_slice(&std::fs::read(repo.gov.join(POD)).unwrap()).unwrap();
        let canary = section(&record, "members")[0]["canary"]
            .as_str()
            .unwrap()
            .to_string();
        if canary != "pending" || std::time::Instant::now() > deadline {
            break canary;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(canary.starts_with("degraded"), "{canary}");
    assert!(canary.contains("did not confirm gh"), "{canary}");
    assert!(canary.contains("pij pod confirm build"), "{canary}");
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(shown["data"]["degraded"][0]["slot"], "lead", "{shown}");
    // The FYI is held just after the canary's commit.
    let told = loop {
        let told = rig
            .services
            .queue
            .pending_fyi_count(&SEAT.into())
            .await
            .unwrap();
        if told > 0 || std::time::Instant::now() > deadline {
            break told;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(told, 1, "the seat that ran up is told");
    rig.server.abort();
}

/// Plan 248 ac-0002, req-04: `--no-start` records the pod and launches
/// nothing; each member then starts on its own with `spawn --as --pod`,
/// recorded as the slot its role fills. A pod with no record refuses the spawn
/// before any launch.
#[tokio::test]
async fn no_start_then_each_member_starts_on_its_own_into_the_pod() {
    let repo = repo("pij-pod-no-start");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--no-start",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    assert_eq!(super::role_start::launches(&rig.tmux), 0);

    let nowhere = super::role_start::spawn(
        rig.addr,
        &repo,
        Some(json!({"role": "notes", "instance": "lead", "set": ["subject=planning"], "pod": "elsewhere"})),
    )
    .await;
    assert_eq!(nowhere["details"]["step"], "finding the pod", "{nowhere}");
    assert_eq!(
        super::role_start::launches(&rig.tmux),
        0,
        "refused before the launch"
    );

    let joined = super::role_start::spawn(
        rig.addr,
        &repo,
        Some(json!({"role": "notes", "instance": "lead", "set": ["subject=planning"], "pod": "build"})),
    )
    .await;
    assert_eq!(joined["ok"], true, "{joined}");
    assert_eq!(joined["details"]["pod"]["slot"], "lead", "{joined}");
    assert_eq!(joined["details"]["pod"]["canary"], "asked", "{joined}");
    let record: Value =
        serde_json::from_slice(&std::fs::read(repo.gov.join(POD)).unwrap()).unwrap();
    let members = section(&record, "members");
    assert_eq!(members[0]["seat"], joined["data"]["id"], "{record}");
    assert_eq!(members[0]["canary"], "pending");
    rig.server.abort();
}

/// ACC-POD-LATEJOIN-FRESH: `pod plan`, then `up --no-start` and the
/// documented join `spawn --as ... --pod`. Answers the plan's first turn for
/// the seeded slot and the first turn the joined seat was given. The role has
/// one memory note, so its index names it.
async fn plan_and_join(name: &str, fresh: bool) -> (String, String) {
    let repo = repo(name);
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let path = repo.main.join("pods/build/pod-template.dd.json");
    let mut seeded: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    seeded["sections"][2]["value"][0]["seed"] = json!("amber-slot: start from the agenda.");
    std::fs::write(&path, serde_json::to_vec_pretty(&seeded).unwrap()).unwrap();
    let noted = directory(
        rig.addr,
        &repo,
        &[
            "memory",
            "add",
            "notes",
            "--subject",
            "planning",
            "--title",
            "Leak marker",
            "--tags",
            "pod",
            "--hook",
            "h",
            "--body",
            "remembered-leak-marker\n",
        ],
    )
    .await;
    assert_eq!(noted["ok"], true, "{noted}");
    let mut argv = vec!["plan", "pods/build", "--set", "subject=planning"];
    if fresh {
        argv.push("--fresh");
    }
    let planned = pod(rig.addr, &repo, &argv).await;
    assert_eq!(planned["ok"], true, "{planned}");
    let planned = planned["data"]["slots"][0]["first_turn"]
        .as_str()
        .unwrap()
        .to_string();
    argv[0] = "up";
    argv.push("--no-start");
    let up = pod(rig.addr, &repo, &argv).await;
    assert_eq!(up["ok"], true, "{up}");
    // The documented join: no --fresh, no --focus, nothing but the slot's
    // role and instance and the pod.
    let joined = super::role_start::spawn(
        rig.addr,
        &repo,
        Some(json!({"role": "notes", "instance": "lead", "pod": "build"})),
    )
    .await;
    assert_eq!(joined["ok"], true, "{joined}");
    assert_eq!(joined["details"]["pod"]["slot"], "lead", "{joined}");
    let seat = joined["data"]["id"].as_str().unwrap();
    let given = std::fs::read_to_string(repo.gov.join(format!(
        ".harness/government/community/roles/notes/first-turns/{seat}.md"
    )))
    .unwrap();
    rig.server.abort();
    (planned, given)
}

/// ACC-POD-LATEJOIN-FRESH: a member joining a `--fresh` pod gets the slot's
/// seed and the fresh placeholder, never the role's memory index; and the
/// turn the plan showed is the turn it got.
#[tokio::test]
async fn a_late_join_into_a_fresh_pod_gets_the_slots_seed_and_no_memory() {
    let (planned, given) = plan_and_join("pij-pod-join-fresh", true).await;
    assert!(given.contains("amber-slot"), "{given}");
    assert!(given.contains("starts fresh"), "{given}");
    assert!(!given.contains("remembered-leak-marker"), "{given}");
    assert!(!given.contains("INDEX.md"), "{given}");
    assert_eq!(given, planned, "the planned turn is the delivered one");
}

/// ACC-POD-LATEJOIN-FRESH: a member joining a pod that is not fresh gets the
/// role's memory index, with the slot's seed, as the plan showed.
#[tokio::test]
async fn a_late_join_into_a_pod_that_is_not_fresh_gets_the_memory_index() {
    let (planned, given) = plan_and_join("pij-pod-join-index", false).await;
    assert!(given.contains("amber-slot"), "{given}");
    assert!(given.contains("INDEX.md"), "{given}");
    assert!(!given.contains("starts fresh"), "{given}");
    assert_eq!(given, planned, "the planned turn is the delivered one");
}

/// ACC-POD-LATEJOIN-FRESH (the reviewer's witness, ported): a join reads the
/// pod record as it stands, not as it was at up: `pod add role:` after a
/// `pod set` gets the new subject, and a role that fills no slot still gets
/// the pod's fresh.
#[tokio::test]
async fn a_join_reads_the_current_record_through_pod_add_and_for_a_role_with_no_slot() {
    let repo = repo("pij-pod-join-current");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=old",
            "--fresh",
            "--no-start",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let set = pod(rig.addr, &repo, &["set", "build", "subject=current"]).await;
    assert_eq!(set["ok"], true, "{set}");
    let first_turn = |seat: &str| {
        std::fs::read_to_string(repo.gov.join(format!(
            ".harness/government/community/roles/notes/first-turns/{seat}.md"
        )))
        .unwrap()
    };
    let added = pod(
        rig.addr,
        &repo,
        &["add", "build", "role:notes/lead", "--session", "work"],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let first = first_turn(added["data"]["added"].as_str().unwrap());
    assert!(first.contains("subject = current"), "{first}");
    assert!(first.contains("starts fresh"), "{first}");
    assert!(!first.contains("INDEX.md"), "{first}");

    let extra = super::role_start::spawn(
        rig.addr,
        &repo,
        Some(json!({"role": "notes", "instance": "extra", "set": ["subject=current"], "pod": "build"})),
    )
    .await;
    assert_eq!(extra["ok"], true, "{extra}");
    let first = first_turn(extra["data"]["id"].as_str().unwrap());
    assert!(first.contains("starts fresh"), "{first}");
    assert!(!first.contains("INDEX.md"), "{first}");
    rig.server.abort();
}

/// ACC-POD-LATEJOIN-FRESH (the reviewer's witness, ported): the slot's seed a
/// join now carries passes the first turn's secret brake, which refuses it
/// before any launch.
#[tokio::test]
async fn a_join_refuses_a_secret_shaped_slot_seed_before_the_launch() {
    let repo = repo("pij-pod-join-seed-brake");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--no-start",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let file = repo.main.join("pods/build/pod-template.dd.json");
    let mut document: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    document["sections"][2]["value"][0]["seed"] = json!("API_KEY=synthetic-example-value");
    std::fs::write(file, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let joined = super::role_start::spawn(
        rig.addr,
        &repo,
        Some(json!({"role": "notes", "instance": "lead", "pod": "build"})),
    )
    .await;
    assert_eq!(
        joined["details"]["code"], "E-RS-FIRST-TURN-SECRET",
        "{joined}"
    );
    assert_eq!(super::role_start::launches(&rig.tmux), 0, "{joined}");
    rig.server.abort();
}

async fn closed_alone(addr: SocketAddr, seat: &str) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/close"))
        .bearer_auth(KEY)
        .json(
            &json!({"seat": seat, "reason": "alone", "stop": true, "caller": {"TMUX_PANE": PANE}}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// Plan 249 ac-0001: a late parameter reaches the members whose slots use
/// it, and only them; show displays it.
#[tokio::test]
async fn pod_set_sends_a_late_parameter_only_to_the_members_that_use_it() {
    let repo = repo("pij-pod-set");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--session",
            "work",
        ],
    )
    .await;
    let reviewer = up["data"]["members"][1]["seat"].clone();
    let set = pod(rig.addr, &repo, &["set", "build", "branch=feature/x"]).await;
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["data"]["sent"], json!([reviewer]), "{set}");
    let unknown = pod(rig.addr, &repo, &["set", "build", "ticket=1"]).await;
    assert_eq!(unknown["details"]["code"], "E-RS-POD-PARAM", "{unknown}");
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert!(
        shown["data"]["parameters"]
            .as_array()
            .unwrap()
            .contains(&json!({"name": "branch", "value": "feature/x"})),
        "{shown}"
    );
    rig.server.abort();
}

/// Plan 249 ac-0002, ac-0003: down records the work folder's branch and head
/// and warns the run is unfrozen; each member that is not keep-open gets the
/// very answer `pij close <seat> --stop` gives alone; the keep-open member is
/// named and left running; the pod reads closing while a member runs.
#[tokio::test]
async fn pod_down_is_close_stop_per_member_spares_keep_open_and_records_the_freeze() {
    let repo = repo("pij-pod-down");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let folder = format!("work-folder={}", repo.main.display());
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--set",
            &folder,
            "--session",
            "work",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let lead = up["data"]["members"][0]["seat"]
        .as_str()
        .unwrap()
        .to_string();
    let reviewer = up["data"]["members"][1]["seat"]
        .as_str()
        .unwrap()
        .to_string();
    let head = git(&repo.main, &["rev-parse", "HEAD"]).trim().to_string();
    let alone = closed_alone(rig.addr, &lead).await;

    let down = pod(rig.addr, &repo, &["down", "build"]).await;
    assert_eq!(down["ok"], true, "{down}");
    let outcome = down["data"]["members"][0]["outcome"].as_str().unwrap();
    assert_eq!(
        outcome,
        format!("not closed: {}", alone["meta"].as_str().unwrap()),
        "the same answer as close --stop alone"
    );
    assert_eq!(
        down["data"]["members"][1]["outcome"],
        "left running (keep-open)"
    );
    let freeze = down["data"]["freeze"].as_str().unwrap();
    assert!(
        freeze.contains(&format!("is on main at {head}")),
        "{freeze}"
    );
    assert!(
        freeze.contains("the run is unfrozen"),
        "pods/ is uncommitted: {freeze}"
    );
    let warnings = down["data"]["warnings"].to_string();
    assert!(
        warnings.contains(&format!("{reviewer} is keep-open")),
        "{warnings}"
    );
    assert!(seat_is_live(&rig, &reviewer).await);
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(
        shown["data"]["state"], "closing",
        "the lead still runs: {shown}"
    );
    let again = pod(rig.addr, &repo, &["up", "build"]).await;
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(again["data"]["members"][0]["outcome"], "running", "{again}");
    rig.server.abort();
}

async fn seat_is_live(rig: &Rig, seat: &str) -> bool {
    rig.services
        .registry
        .get(&seat.into())
        .await
        .unwrap()
        .is_some_and(|row| row.tombstoned_at.is_none())
}

/// Plan 249 ac-0004, req-08: a running seat joins a pod and leaves it, each
/// logged; the template is unchanged.
#[tokio::test]
async fn a_running_seat_joins_and_leaves_a_pod_and_the_template_is_unchanged() {
    let repo = repo("pij-pod-reshape");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let before = std::fs::read(repo.main.join("pods/build/pod-template.dd.json")).unwrap();
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--no-start",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let added = pod(rig.addr, &repo, &["add", "build", SEAT]).await;
    assert_eq!(added["ok"], true, "{added}");
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(shown["data"]["members"][0]["seat"], SEAT, "{shown}");
    let twice = pod(rig.addr, &repo, &["add", "build", SEAT]).await;
    assert_eq!(twice["ok"], false, "{twice}");
    let released = pod(rig.addr, &repo, &["release", "build", SEAT]).await;
    assert_eq!(released["ok"], true, "{released}");
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(shown["data"]["members"], json!([]), "{shown}");
    let log = shown["data"]["log"].to_string();
    assert!(
        log.contains(&format!("added {SEAT}")) && log.contains(&format!("released {SEAT}")),
        "{log}"
    );
    assert!(seat_is_live(&rig, SEAT).await, "release leaves it running");
    assert_eq!(
        std::fs::read(repo.main.join("pods/build/pod-template.dd.json")).unwrap(),
        before
    );
    rig.server.abort();
}

/// Plan 250 ac-0002, ac-0003: `pij sessions --history` and `pij pod sessions`
/// read the seat's facts back from the event log, show a session whose
/// transcript is gone as context unknown, and launch nothing.
#[tokio::test]
async fn session_history_reads_the_event_log_per_seat_and_per_member() {
    let repo = repo("pij-pod-sessions");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--session",
            "work",
        ],
    )
    .await;
    let lead = up["data"]["members"][0]["seat"]
        .as_str()
        .unwrap()
        .to_string();
    for session in ["pij-history-s1", "pij-history-s2"] {
        let mut row = rig
            .services
            .registry
            .get(&lead.as_str().into())
            .await
            .unwrap()
            .unwrap();
        row.harness_session = Some(session.to_string());
        rig.services.registry.put(row).await.unwrap();
    }
    let launched = super::role_start::launches(&rig.tmux);

    let history: Value = reqwest::Client::new()
        .get(format!(
            "http://{}/v1/shim/sessions?history=true&seat={lead}",
            rig.addr
        ))
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let seat = &history["data"]["history"][0];
    assert_eq!(seat["total"], 2, "{history}");
    assert_eq!(
        seat["sessions"][0]["session"], "pij-history-s2",
        "newest first: {history}"
    );
    assert_eq!(
        seat["sessions"][0]["why"], "relaunch",
        "no spawn or revive beside it"
    );
    assert_eq!(seat["sessions"][1]["why"], "spawn");
    assert_eq!(
        seat["sessions"][1]["ended_at"],
        seat["sessions"][0]["started_at"]
    );
    assert_eq!(
        seat["sessions"][0]["context_tokens"], "unknown",
        "no transcript"
    );

    let members = pod(rig.addr, &repo, &["sessions", "build", "--limit", "1"]).await;
    assert_eq!(members["ok"], true, "{members}");
    assert_eq!(members["data"]["sessions"][0]["slot"], "lead");
    // ACC-POD-SESSIONS-PROJECT: the same project `pod show` names.
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert!(members["data"]["project"].is_string(), "{members}");
    assert_eq!(members["data"]["project"], shown["data"]["project"]);
    assert_eq!(
        members["data"]["sessions"][0]["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        super::role_start::launches(&rig.tmux),
        launched,
        "reads launch nothing"
    );
    rig.server.abort();
}

/// Plan 251: a seat in no pod and no role there reads what the pod did, in
/// order, with each member's transcript location; nothing launches.
#[tokio::test]
async fn pod_activity_is_open_to_any_seat_and_reads_in_time_order() {
    let repo = repo("pij-pod-activity");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--session",
            "work",
        ],
    )
    .await;
    let lead = up["data"]["members"][0]["seat"]
        .as_str()
        .unwrap()
        .to_string();
    rig.delivery
        .send(SEAT.into(), lead.as_str().into(), "hello lead\nsecond line")
        .await
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    rig.bus
        .publish(pij_core::model::Event {
            seq: None,
            v: 1,
            at: u64::try_from(now).unwrap() + 1,
            kind: "seat.activity".to_string(),
            seat: Some(lead.as_str().into()),
            payload: json!({"state": "idle"}).to_string(),
        })
        .await
        .unwrap();
    let launched = super::role_start::launches(&rig.tmux);

    let seen = pod(rig.addr, &repo, &["activity", "build"]).await;
    assert_eq!(seen["ok"], true, "{seen}");
    let lines: Vec<&str> = seen["data"]["activity"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["seat"] == lead.as_str())
        .map(|line| line["line"].as_str().unwrap())
        .collect();
    let hello = lines
        .iter()
        .position(|line| *line == format!("{SEAT} -> {lead}: hello lead…"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    let idle = lines
        .iter()
        .position(|line| *line == format!("{lead} is idle"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert!(hello < idle, "in time order: {lines:?}");
    assert_eq!(seen["data"]["transcripts"][0]["seat"], lead.as_str());
    let later = pod(
        rig.addr,
        &repo,
        &["activity", "build", "--since", "4102444800000"],
    )
    .await;
    assert_eq!(later["data"]["activity"], json!([]), "{later}");
    assert_eq!(super::role_start::launches(&rig.tmux), launched);
    rig.server.abort();
}

/// Plan 252, req-16, req-17: the first stage is recorded from the members it
/// needs while a late one keeps working, and show names what the next stage
/// waits on; each stage is written back to the role the template names as
/// one request, which stays open after the pod goes down.
#[tokio::test]
async fn stages_never_wait_on_a_late_member_and_each_is_written_back_as_a_request() {
    let repo = repo("pij-pod-stages");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    let added = directory(
        rig.addr,
        &repo,
        &[
            "add",
            "keeper",
            "--type",
            "knowledge",
            "--title",
            "Keeper",
            "--holders",
            "one",
            "--keeps",
            "notes/area.md",
            "--why",
            "w",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    template(&repo);
    let file = repo.main.join("pods/build/pod-template.dd.json");
    let mut document: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    document["sections"][4]["value"] = json!([{
        "id": "report", "owner": "keeper", "stages": ["draft", "final"], "write_back": "keeper",
        "stage_needs": [{"stage": "draft", "slots": ["lead"]}],
    }]);
    std::fs::write(&file, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--session",
            "work",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");

    let draft = pod(
        rig.addr,
        &repo,
        &[
            "stage",
            "build",
            "report",
            "draft",
            "--output",
            "out/draft.md",
        ],
    )
    .await;
    assert_eq!(draft["ok"], true, "{draft}");
    assert_eq!(draft["data"]["from"], json!(["lead"]));
    assert_eq!(
        draft["data"]["warnings"],
        json!([]),
        "draft needs only the lead: {draft}"
    );
    let request = draft["data"]["write_back"].as_str().unwrap().to_string();
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    let stages = &shown["data"]["deliverables"][0]["stages"];
    assert_eq!(stages[0]["recorded"], true, "{shown}");
    assert_eq!(
        stages[1]["waits_on"],
        json!(["reviewer"]),
        "lead is folded in: {shown}"
    );

    let early = pod(
        rig.addr,
        &repo,
        &[
            "stage",
            "build",
            "report",
            "final",
            "--output",
            "out/final.md",
            "--from",
            "lead",
        ],
    )
    .await;
    assert_eq!(early["ok"], true, "a late member never blocks: {early}");
    assert!(
        early["data"]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("without the work of reviewer"),
        "{early}"
    );
    let twice = pod(
        rig.addr,
        &repo,
        &["stage", "build", "report", "final", "--output", "x"],
    )
    .await;
    assert_eq!(twice["ok"], false, "{twice}");
    let log = pod(rig.addr, &repo, &["show", "build"]).await["data"]["log"].to_string();
    assert!(log.contains(&format!("written back as {request}")), "{log}");

    let requests = [
        request,
        early["data"]["write_back"].as_str().unwrap().to_string(),
    ];
    assert_ne!(requests[0], requests[1], "one request per stage");
    pod(rig.addr, &repo, &["down", "build"]).await;
    for id in &requests {
        let found = rig.store.role_request(id).await.unwrap().unwrap();
        assert_eq!(found.role, "keeper");
        assert_eq!(
            found.state,
            pij_store::RequestState::Queued,
            "it outlives the pod: {id}"
        );
    }
    rig.server.abort();
}

// --- #346 review round 1 (B346-R1-01..09): each finding's regression.

async fn no_start(rig: &Rig, repo: &Repo, extra: &[&str]) -> Value {
    let mut argv = vec![
        "up",
        "pods/build",
        "--set",
        "subject=planning",
        "--no-start",
    ];
    argv.extend_from_slice(extra);
    pod(rig.addr, repo, &argv).await
}

async fn confirm_as(addr: SocketAddr, repo: &Repo, seat: &str, tools: &[&str]) -> Value {
    let mut argv = vec!["pod", "confirm", "build"];
    for tool in tools {
        argv.extend_from_slice(&["--tool", tool]);
    }
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/pod"))
        .bearer_auth(KEY)
        .json(&json!({"argv": argv, "caller": {"PIJ_SESSION_ID": seat, "cwd": repo.main}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// The lead's canary once it is no longer pending (or the bound passes).
async fn settled_canary(repo: &Repo) -> String {
    let deadline = std::time::Instant::now() + scaled(std::time::Duration::from_secs(10));
    loop {
        let record: Value =
            serde_json::from_slice(&std::fs::read(repo.gov.join(POD)).unwrap()).unwrap();
        let canary = section(&record, "members")[0]["canary"]
            .as_str()
            .unwrap()
            .to_string();
        if canary != "pending" || std::time::Instant::now() > deadline {
            return canary;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn up_started(rig: &Rig, repo: &Repo) -> (String, String) {
    let up = pod(
        rig.addr,
        repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--session",
            "work",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let seat = |index: usize| {
        up["data"]["members"][index]["seat"]
            .as_str()
            .unwrap()
            .to_string()
    };
    (seat(0), seat(1))
}

/// B346-R1-01: a secret-shaped `--set` value, used or not, is refused before
/// anything is committed, at up (even --no-start) and at set; the refusal
/// names the parameter and never the value. Mutant: make
/// `refuse_secret_parameters` return Ok and both commits land.
#[tokio::test]
async fn a_secret_shaped_parameter_is_refused_before_any_commit() {
    let repo = repo("pij-pod-secret");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let before = head(&repo);
    let secret = "branch=API_TOKEN=abcd1234EFGH5678";
    let refused = no_start(&rig, &repo, &["--set", secret]).await;
    assert_eq!(
        refused["details"]["code"], "E-RS-FIRST-TURN-SECRET",
        "{refused}"
    );
    assert!(
        refused["meta"].as_str().unwrap().contains("--set branch"),
        "{refused}"
    );
    assert!(
        !refused.to_string().contains("abcd1234EFGH5678"),
        "{refused}"
    );
    assert_eq!(head(&repo), before, "nothing committed");

    assert_eq!(no_start(&rig, &repo, &[]).await["ok"], true);
    let created = head(&repo);
    let late = pod(rig.addr, &repo, &["set", "build", secret]).await;
    assert_eq!(late["details"]["code"], "E-RS-FIRST-TURN-SECRET", "{late}");
    assert_eq!(head(&repo), created, "set committed nothing");
    rig.server.abort();
}

/// B346-R1-02: two sets at once both survive: each update reads the record
/// inside the writer's serialized build. Mutant: give `mutate`'s change the
/// record read before the write and one value is lost.
#[tokio::test]
async fn two_concurrent_sets_both_survive() {
    let repo = repo("pij-pod-concurrent");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    assert_eq!(no_start(&rig, &repo, &[]).await["ok"], true);
    let (branch, subject) = tokio::join!(
        pod(rig.addr, &repo, &["set", "build", "branch=feature/x"]),
        pod(rig.addr, &repo, &["set", "build", "subject=updated"]),
    );
    assert_eq!(branch["ok"], true, "{branch}");
    assert_eq!(subject["ok"], true, "{subject}");
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    let parameters = shown["data"]["parameters"].as_array().unwrap();
    for expected in [
        json!({"name": "branch", "value": "feature/x"}),
        json!({"name": "subject", "value": "updated"}),
    ] {
        assert!(parameters.contains(&expected), "{expected}: {shown}");
    }
    assert_eq!(
        shown["data"]["meta"]["subject"], "updated",
        "one source (R1-06)"
    );
    rig.server.abort();
}

/// B346-R1-03: the printed fix works after the canary settled: confirming
/// every tool brings the degraded slot back, so pod work reaches it again.
#[tokio::test]
async fn confirming_after_the_canary_settled_recovers_a_degraded_slot() {
    let repo = repo("pij-pod-recover");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let (lead, _) = up_started(&rig, &repo).await;
    assert_eq!(
        confirm_as(rig.addr, &repo, &lead, &["git"]).await["ok"],
        true
    );
    assert!(settled_canary(&repo).await.starts_with("degraded"));
    let fixed = confirm_as(rig.addr, &repo, &lead, &["git", "gh"]).await;
    assert_eq!(fixed["data"]["canary"], "confirmed", "{fixed}");
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(shown["data"]["degraded"], json!([]), "{shown}");
    let set = pod(rig.addr, &repo, &["set", "build", "subject=again"]).await;
    assert!(
        set["data"]["sent"]
            .as_array()
            .unwrap()
            .contains(&json!(lead)),
        "{set}"
    );
    rig.server.abort();
}

/// B346-R1-04: a template named relative to the caller's folder is recorded
/// as resolved, so a later verb asked from elsewhere reads the same one.
#[tokio::test]
async fn a_caller_relative_template_is_read_again_from_anywhere() {
    let repo = repo("pij-pod-relative");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let up = call(
        rig.addr,
        "/v1/pod",
        &[
            "pod",
            "up",
            "build",
            "--set",
            "subject=planning",
            "--no-start",
        ],
        &repo.main.join("pods"),
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let set = pod(rig.addr, &repo, &["set", "build", "branch=feature/x"]).await;
    assert_eq!(set["ok"], true, "{set}");
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(shown["data"]["meta"]["template"], "pods/build", "{shown}");
    rig.server.abort();
}

/// B346-R1-05: a seat in one pod is refused by a second, naming the first
/// and the release that frees it.
#[tokio::test]
async fn a_seat_in_one_pod_is_refused_by_a_second() {
    let repo = repo("pij-pod-two");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    assert_eq!(no_start(&rig, &repo, &[]).await["ok"], true);
    assert_eq!(
        no_start(&rig, &repo, &["--name", "other"]).await["ok"],
        true
    );
    assert_eq!(
        pod(rig.addr, &repo, &["add", "build", SEAT]).await["ok"],
        true
    );
    let second = pod(rig.addr, &repo, &["add", "other", SEAT]).await;
    assert_eq!(second["details"]["code"], "E-RS-POD-MEMBER", "{second}");
    assert!(
        second["meta"]
            .as_str()
            .unwrap()
            .contains(&format!("pij pod release build {SEAT}")),
        "{second}"
    );
    assert_eq!(
        pod(rig.addr, &repo, &["release", "build", SEAT]).await["ok"],
        true
    );
    assert_eq!(
        pod(rig.addr, &repo, &["add", "other", SEAT]).await["ok"],
        true
    );
    rig.server.abort();
}

/// B346-R1-06: a work folder set after up drives down's freeze.
#[tokio::test]
async fn a_late_work_folder_drives_the_freeze() {
    let repo = repo("pij-pod-late-folder");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    assert_eq!(no_start(&rig, &repo, &[]).await["ok"], true);
    let folder = format!("work-folder={}", repo.main.display());
    let set = pod(rig.addr, &repo, &["set", "build", &folder]).await;
    assert_eq!(set["ok"], true, "{set}");
    let down = pod(rig.addr, &repo, &["down", "build"]).await;
    assert!(
        down["data"]["freeze"]
            .as_str()
            .is_some_and(|line| line.contains("is on main at")),
        "{down}"
    );
    rig.server.abort();
}

/// B346-R1-07: a write-back to a role a degraded member holds is not sent;
/// the stage is still recorded and the member named. Mutant: drop the
/// degraded-holder check and a request is created.
#[tokio::test]
async fn a_write_back_to_a_degraded_member_is_held_by_the_brake() {
    let repo = repo("pij-pod-write-back-brake");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let file = repo.main.join("pods/build/pod-template.dd.json");
    let mut document: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    document["sections"][4]["value"] =
        json!([{"id": "report", "owner": "notes", "write_back": "notes"}]);
    std::fs::write(&file, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let (lead, _) = up_started(&rig, &repo).await;
    assert_eq!(
        confirm_as(rig.addr, &repo, &lead, &["git"]).await["ok"],
        true
    );
    assert!(settled_canary(&repo).await.starts_with("degraded"));
    let staged = pod(
        rig.addr,
        &repo,
        &["stage", "build", "report", "done", "--output", "out/r.md"],
    )
    .await;
    assert_eq!(staged["ok"], true, "{staged}");
    assert_eq!(staged["data"]["write_back"], Value::Null, "{staged}");
    assert!(
        staged["data"]["warnings"]
            .to_string()
            .contains(&format!("its holder {lead} is a degraded member")),
        "{staged}"
    );
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(
        shown["data"]["deliverables"][0]["stages"][0]["recorded"], true,
        "{shown}"
    );
    rig.server.abort();
}

/// B346-R1-09: a canary that settles after the pod went down writes and
/// sends nothing.
#[tokio::test]
async fn a_canary_settling_after_down_writes_and_sends_nothing() {
    let repo = repo("pij-pod-late-canary");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let (lead, _) = up_started(&rig, &repo).await;
    assert_eq!(pod(rig.addr, &repo, &["down", "build"]).await["ok"], true);
    let downed = head(&repo);
    assert_eq!(
        confirm_as(rig.addr, &repo, &lead, &["git"]).await["ok"],
        true
    );
    tokio::time::sleep(scaled(std::time::Duration::from_millis(500))).await;
    assert_eq!(head(&repo), downed, "no settlement commit");
    let told = rig
        .services
        .queue
        .pending_fyi_count(&SEAT.into())
        .await
        .unwrap();
    assert_eq!(told, 0, "no degraded FYI for a pod that is down");
    rig.server.abort();
}

/// Plan 253 ac-0001, ac-0002: a template names its own procedures and links
/// the repo's; `plan` lists each with whether it is there and warns, with the
/// fix, about one that is not; `show` names the close procedure the pij CLI
/// runs before down; down records whether it ran, was skipped, or did not run.
#[tokio::test]
async fn a_template_names_its_procedures_and_down_records_its_close_procedure() {
    let repo = repo("pij-pod-procedures");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let file = repo.main.join("pods/build/pod-template.dd.json");
    let mut document: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    document["sections"].as_array_mut().unwrap().push(json!({
        "name": "procedures",
        "value": [
            {"id": "PROC-pod-0001", "from": "template", "use": "close"},
            {"id": "PROC-fleet-0002", "from": "repo", "use": "up"},
            {"id": "PROC-meta-0001", "from": "repo"},
        ],
    }));
    std::fs::write(&file, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let own = repo
        .main
        .join("pods/build/procedures/pod/PROC-pod-0001-close-the-run");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("procedure.dd.json"), "{}").unwrap();

    let planned = pod(
        rig.addr,
        &repo,
        &["plan", "pods/build", "--set", "subject=planning"],
    )
    .await;
    assert_eq!(planned["ok"], true, "{planned}");
    let present: Vec<(String, bool)> = planned["data"]["procedures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap().to_string(),
                row["present"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        present,
        [
            ("PROC-pod-0001".to_string(), true),
            ("PROC-fleet-0002".to_string(), false),
            ("PROC-meta-0001".to_string(), true),
        ],
        "the template's own is in its folder; a skill area ships with pij: {planned}"
    );
    assert!(
        abouts(&planned).contains(&"procedure PROC-fleet-0002".to_string()),
        "{planned}"
    );

    let up = pod(
        rig.addr,
        &repo,
        &[
            "up",
            "pods/build",
            "--set",
            "subject=planning",
            "--no-start",
        ],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    let close = &shown["data"]["close_procedure"];
    assert_eq!(close["id"], "PROC-pod-0001", "{shown}");
    assert_eq!(close["key"], "pod-build");
    assert_eq!(
        std::fs::canonicalize(close["template"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(repo.main.join("pods/build")).unwrap()
    );

    let down = pod(rig.addr, &repo, &["down", "build"]).await;
    assert_eq!(down["ok"], true, "{down}");
    assert!(
        down["data"]["warnings"]
            .to_string()
            .contains("close procedure PROC-pod-0001 did not run"),
        "{down}"
    );
    let ran = pod(rig.addr, &repo, &["down", "build", "--close-procedure-ran"]).await;
    assert_eq!(ran["ok"], true, "{ran}");
    let skipped = pod(
        rig.addr,
        &repo,
        &["down", "build", "--skip-close-procedure"],
    )
    .await;
    assert_eq!(skipped["ok"], true, "{skipped}");
    let log = pod(rig.addr, &repo, &["show", "build"]).await["data"]["log"].to_string();
    for line in [
        "close procedure PROC-pod-0001 not run",
        "close procedure PROC-pod-0001 ran (key pod-build)",
        "close procedure PROC-pod-0001 skipped (--skip-close-procedure)",
    ] {
        assert!(log.contains(line), "{line}: {log}");
    }

    // B347-R1-01: a template moved away is not "no close procedure".
    std::fs::rename(repo.main.join("pods/build"), repo.main.join("pods/moved")).unwrap();
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert!(
        shown["data"]["close_procedure"]["error"].is_string(),
        "{shown}"
    );
    let refused = pod(rig.addr, &repo, &["down", "build"]).await;
    assert_eq!(refused["details"]["code"], "E-RS-POD-TEMPLATE", "{refused}");
    assert!(
        refused["meta"]
            .as_str()
            .unwrap()
            .contains("--skip-close-procedure"),
        "{refused}"
    );
    let skipped = pod(
        rig.addr,
        &repo,
        &["down", "build", "--skip-close-procedure"],
    )
    .await;
    assert_eq!(skipped["ok"], true, "{skipped}");
    let log = pod(rig.addr, &repo, &["show", "build"]).await["data"]["log"].to_string();
    assert!(
        log.contains("close procedure unknown, skipped (--skip-close-procedure)"),
        "{log}"
    );
    rig.server.abort();
}

/// B347-R1-04: a template's procedures holding a link out of its folder
/// refuse export by the link's path, never its content, and nothing is
/// written or committed; a brake on what pij copies.
#[tokio::test]
async fn an_export_refuses_a_procedure_link_out_of_the_template() {
    let repo = repo("pij-pod-export-link");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let own = repo
        .main
        .join("pods/build/procedures/pod/PROC-pod-0001-close");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("procedure.dd.json"), "{}").unwrap();
    let outside = repo.main.join("outside-marker.txt");
    std::fs::write(&outside, "MARKER-NOT-TO-BE-COPIED").unwrap();
    std::os::unix::fs::symlink(&outside, own.join("config.txt")).unwrap();
    up_started(&rig, &repo).await;
    let before = head(&repo);
    for target in [
        "pods/out",
        ".harness/government/community/pod-templates/out",
    ] {
        let cwd_target = if target.starts_with(".harness") {
            repo.gov.join(target).display().to_string()
        } else {
            target.to_string()
        };
        let refused = pod(
            rig.addr,
            &repo,
            &[
                "export",
                "build",
                "--as-template",
                &cwd_target,
                "--why",
                "w",
            ],
        )
        .await;
        assert_eq!(refused["details"]["code"], "E-RS-POD-TEMPLATE", "{refused}");
        let text = refused.to_string();
        assert!(text.contains("config.txt"), "names the link: {text}");
        assert!(!text.contains("MARKER-NOT-TO-BE-COPIED"), "{text}");
    }
    assert!(!repo.main.join("pods/out").exists());
    assert_eq!(head(&repo), before, "nothing committed");
    rig.server.abort();
}

/// Plan 254 ac-0001..ac-0005 (PRD 010 req-02, req-03, req-06): a shipped
/// template is named as `shipped:<name>`, recorded at its version, adopted to
/// governance (one commit) or a path (files), read by that path, diffed
/// against upstream without being changed, and `community init` copies the
/// ten seeds and the named templates, keeping what is there. Nothing
/// launches a seat.
#[tokio::test]
async fn shipped_templates_are_named_adopted_diffed_and_set_up_by_init() {
    let repo = repo("pij-pod-shipped");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;

    let listed = pod(rig.addr, &repo, &["templates"]).await;
    assert_eq!(listed["data"]["templates"][0]["name"], "build", "{listed}");
    let planned = pod(
        rig.addr,
        &repo,
        &["plan", "shipped:build", "--set", "subject=step"],
    )
    .await;
    assert_eq!(planned["ok"], true, "{planned}");
    assert_eq!(planned["data"]["template"]["source"], "shipped:build@v1");
    let up = pod(
        rig.addr,
        &repo,
        &["up", "shipped:build", "--set", "subject=step", "--no-start"],
    )
    .await;
    assert_eq!(up["ok"], true, "{up}");
    let set = pod(rig.addr, &repo, &["set", "build", "subject=again"]).await;
    assert_eq!(set["ok"], true, "a shipped template is read again: {set}");

    let before = head(&repo);
    let into = pod(
        rig.addr,
        &repo,
        &["adopt", "build", "--into", "pods/mine", "--why", "ours"],
    )
    .await;
    assert_eq!(into["ok"], true, "{into}");
    assert_eq!(
        into["data"]["commit"],
        Value::Null,
        "files in the repo, no commit"
    );
    assert_eq!(head(&repo), before);
    let file = repo.main.join("pods/mine/pod-template.dd.json");
    let copy: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(section(&copy, "meta")["from"], "shipped:build@v1");
    let again = pod(
        rig.addr,
        &repo,
        &["adopt", "build", "--into", "pods/mine", "--why", "ours"],
    )
    .await;
    assert_eq!(again["ok"], false, "never overwritten: {again}");
    let governed = pod(rig.addr, &repo, &["adopt", "build", "--why", "ours"]).await;
    assert_eq!(governed["ok"], true, "{governed}");
    let path = governed["data"]["wrote"][0]["path"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(path, ".harness/government/community/pod-templates/build");
    assert!(
        repo.gov.join(&path).join("pod-template.dd.json").is_file(),
        "{governed}"
    );
    assert_ne!(head(&repo), before, "one governance commit");

    let mine = pod(
        rig.addr,
        &repo,
        &[
            "plan",
            "pods/mine",
            "--set",
            "subject=step",
            "--name",
            "mine",
        ],
    )
    .await;
    assert_eq!(mine["data"]["template"]["source"], "pods/mine", "{mine}");
    let edited = std::fs::read(&file).unwrap();
    let diffed = pod(rig.addr, &repo, &["template-diff", "pods/mine"]).await;
    assert_eq!(diffed["data"]["from"], "shipped:build@v1", "{diffed}");
    assert_eq!(diffed["data"]["current"], true, "{diffed}");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        edited,
        "diff applies nothing"
    );

    let init = call(
        rig.addr,
        "/v1/pod",
        &[
            "community",
            "init",
            "--seeds",
            "all",
            "--templates",
            "build",
            "--why",
            "setup",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(init["ok"], true, "{init}");
    assert_eq!(
        init["data"]["wrote"].as_array().unwrap().len(),
        10,
        "{init}"
    );
    assert_eq!(
        init["data"]["kept"][0]["path"], ".harness/government/community/pod-templates/build",
        "{init}"
    );
    let twice = call(
        rig.addr,
        "/v1/pod",
        &["community", "init", "--seeds", "all", "--why", "setup"],
        &repo.main,
    )
    .await;
    assert_eq!(twice["data"]["commit"], Value::Null, "{twice}");
    assert_eq!(twice["data"]["kept"].as_array().unwrap().len(), 10);
    let elsewhere = call(
        rig.addr,
        "/v1/pod",
        &[
            "community",
            "init",
            "--templates",
            "build",
            "--into",
            "tpl",
            "--why",
            "setup",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(elsewhere["ok"], true, "{elsewhere}");
    assert!(repo.main.join("tpl/build/pod-template.dd.json").is_file());
    let nothing = call(
        rig.addr,
        "/v1/pod",
        &["community", "init", "--why", "x"],
        &repo.main,
    )
    .await;
    assert_eq!(
        nothing["ok"], false,
        "pij copies nothing on its own: {nothing}"
    );

    assert_eq!(
        super::role_start::launches(&rig.tmux),
        0,
        "no seat launched"
    );
    rig.server.abort();
}

/// Plan 255 ac-0001..ac-0003 (PRD 010 req-04): a pod with a seat added while
/// it ran exports to a template with that extra slot and no seat id, which
/// `plan` reads; to governance it is one commit. A role saves as the repo's
/// own role template, and a role is made from it. A role shape made twice
/// by hand is suggested in the view. Nothing launches.
#[tokio::test]
async fn a_pod_and_a_role_export_as_templates_and_a_repeated_shape_is_suggested() {
    let repo = repo("pij-pod-export");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    super::role_start::add_notes(rig.addr, &repo).await;
    template(&repo);
    let (lead, reviewer) = up_started(&rig, &repo).await;
    let launched = super::role_start::launches(&rig.tmux);
    assert_eq!(
        pod(rig.addr, &repo, &["add", "build", SEAT]).await["ok"],
        true
    );

    let exported = pod(
        rig.addr,
        &repo,
        &[
            "export",
            "build",
            "--as-template",
            "pods/grown",
            "--why",
            "it worked",
        ],
    )
    .await;
    assert_eq!(exported["ok"], true, "{exported}");
    assert_eq!(exported["data"]["commit"], Value::Null, "files in the repo");
    let text = std::fs::read_to_string(repo.main.join("pods/grown/pod-template.dd.json")).unwrap();
    for seat in [lead.as_str(), reviewer.as_str(), SEAT] {
        assert!(!text.contains(seat), "no seat id: {seat}: {text}");
    }
    let planned = pod(
        rig.addr,
        &repo,
        &[
            "plan",
            "pods/grown",
            "--set",
            "subject=again",
            "--name",
            "again",
        ],
    )
    .await;
    assert_eq!(planned["ok"], true, "{planned}");
    assert_eq!(
        planned["data"]["slots"].as_array().unwrap().len(),
        3,
        "lead, reviewer and the seat added while it ran: {planned}"
    );
    let before = head(&repo);
    let governed = repo
        .gov
        .join(".harness/government/community/pod-templates/grown");
    let into_governance = pod(
        rig.addr,
        &repo,
        &[
            "export",
            "build",
            "--as-template",
            governed.to_str().unwrap(),
            "--why",
            "keep it",
        ],
    )
    .await;
    assert_eq!(into_governance["ok"], true, "{into_governance}");
    assert!(
        into_governance["data"]["commit"].is_string(),
        "{into_governance}"
    );
    assert_ne!(head(&repo), before);
    assert!(governed.join("pod-template.dd.json").is_file());

    let saved = directory(
        rig.addr,
        &repo,
        &[
            "export",
            "notes",
            "--as-template",
            "roles/notes",
            "--why",
            "reuse it",
        ],
    )
    .await;
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["data"]["parameters"][0]["name"], "subject", "{saved}");
    let made = directory(
        rig.addr,
        &repo,
        &[
            "add",
            "notes-two",
            "--file",
            "roles/notes/role.dd.json",
            "--why",
            "from our template",
        ],
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");

    let view = directory(rig.addr, &repo, &[]).await;
    let save = &view["data"]["save_as_template"];
    assert_eq!(save[0]["roles"], json!(["notes", "notes-two"]), "{view}");
    assert!(
        save[0]["command"]
            .as_str()
            .unwrap()
            .starts_with("pij directory export notes --as-template"),
        "{save}"
    );
    assert_eq!(
        super::role_start::launches(&rig.tmux),
        launched,
        "export launches nothing"
    );
    rig.server.abort();
}

/// Plan 256 ac-0001..ac-0003: on a project with their roles, the shipped
/// build and live-call pods each plan with no warning (the live call's lead
/// live, its procedures pij's own), warn once the reviewing slot shares its
/// author's model, and each stands up and comes down.
#[tokio::test]
async fn the_shipped_pods_plan_clean_then_stand_up_and_come_down() {
    let repo = repo("pij-pod-shipped-two");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    for (role, kind, model) in [
        ("pm", "project-manager", "model-a"),
        ("coder", "coder", "model-a"),
        ("reviewer", "reviewer", "model-b"),
        ("note-taker", "worker", "model-a"),
        ("researcher", "worker", "model-a"),
        ("validator", "reviewer", "model-b"),
        ("subject-lead", "worker", "model-a"),
        ("subject-knowledge", "worker", "model-a"),
    ] {
        let added = super::directory(
            rig.addr,
            &repo,
            &[
                "add",
                role,
                "--type",
                kind,
                "--title",
                role,
                "--holders",
                "many",
                "--rule",
                "r",
                "--packet",
                "p",
                "--harness",
                "omp",
                "--model",
                model,
                "--why",
                "w",
            ],
        )
        .await;
        assert_eq!(added["ok"], true, "{added}");
    }
    let listed = pod(rig.addr, &repo, &["templates"]).await;
    let names: Vec<&str> = listed["data"]["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["build", "live-call"], "{listed}");
    let lead =
        super::role_start::spawn(rig.addr, &repo, Some(json!({"role": "subject-lead"}))).await;
    assert_eq!(lead["ok"], true, "{lead}");

    let build = ["shipped:build", "--set", "subject=step"];
    let call = [
        "shipped:live-call",
        "--set",
        "subject=billing",
        "--set",
        "meeting=standup",
    ];
    for argv in [&build[..], &call[..]] {
        let planned = pod(rig.addr, &repo, &[&["plan"], argv].concat()).await;
        assert_eq!(planned["ok"], true, "{planned}");
        assert_eq!(abouts(&planned), Vec::<String>::new(), "{planned}");
        assert_eq!(planned["data"]["slots"].as_array().unwrap().len(), 3);
    }
    for (role, argv, slot) in [
        ("reviewer", &build[..], "slot reviewer"),
        ("validator", &call[..], "slot validator"),
    ] {
        let edited = super::directory(
            rig.addr,
            &repo,
            &["edit", role, "--model", "model-a", "--why", "w"],
        )
        .await;
        assert_eq!(edited["ok"], true, "{edited}");
        let planned = pod(rig.addr, &repo, &[&["plan"], argv].concat()).await;
        assert_eq!(abouts(&planned), [slot], "its author's model: {planned}");
    }

    for (argv, name) in [(&build[..], "build"), (&call[..], "live-call")] {
        let up = pod(
            rig.addr,
            &repo,
            &[&["up"], argv, &["--session", "work"]].concat(),
        )
        .await;
        assert_eq!(up["ok"], true, "{up}");
        assert_eq!(up["data"]["members"].as_array().unwrap().len(), 3, "{up}");
        let down = pod(rig.addr, &repo, &["down", name]).await;
        assert_eq!(down["ok"], true, "{down}");
        let shown = pod(rig.addr, &repo, &["show", name]).await;
        assert_ne!(shown["data"]["state"], "up", "{shown}");
    }
    let shown = pod(rig.addr, &repo, &["show", "build"]).await;
    assert_eq!(shown["data"]["close_procedure"]["id"], "PROC-pods-0002");
    assert_eq!(
        shown["data"]["close_procedure"]["template"],
        Value::Null,
        "a shipped template has no folder: {shown}"
    );
    assert_eq!(shown["data"]["close_procedure"]["present"], true);
    rig.server.abort();
}
