use pij_core::model::Pane;
use pij_harnesses::InteractionGate;
use pij_testkit::block_on;
use pij_testkit::fakes::FakeTmux;
use std::sync::Arc;

#[test]
fn captured_claude_281_empty_composers_are_not_deferred() {
    for (id, recorded) in [
        ("%486", include_str!("fixtures/incident155/pane-486.txt")),
        ("%495", include_str!("fixtures/incident155/pane-495.txt")),
    ] {
        let mut lines: Vec<_> = recorded.lines().collect();
        let metadata: Vec<_> = lines.pop().unwrap().split_whitespace().collect();
        let tmux = Arc::new(
            FakeTmux::new()
                .with_attached_tap(id)
                .with_pane(Pane {
                    id: id.to_owned(),
                    session: "incident".into(),
                    window: "probe".into(),
                    title: "".into(),
                    cursor_x: Some(metadata[1].parse().unwrap()),
                    cursor_y: Some(metadata[2].parse().unwrap()),
                })
                .script_capture(lines.join("\n")),
        );
        let gate = InteractionGate::new(tmux);
        let verdict = block_on(gate.fresh_injection_verdict(id)).unwrap();
        eprintln!("{id}: {verdict:?}");
        assert!(
            verdict.permitted && verdict.composer_idle,
            "{id}: {verdict:?}"
        );
        assert_eq!(verdict.reason, None);
    }
}
