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
