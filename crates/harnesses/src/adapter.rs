use crate::composer::is_codex_footer;
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use pij_core::admission::{Admission, Candidate, admit};
use pij_core::error::{PijError, Result};
use pij_core::model::{BindHealth, Harness, ModelRow, Readiness, SeatDescriptor};
use pij_core::ports::{HarnessPort, TmuxPort};

const CAPTURE_LINES: u32 = 200;

/// Facts a harness adapter observed rather than copied from the registry row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedBindFacts {
    pub native_session_id: Option<String>,
    pub pane: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub readiness: Readiness,
}

/// The shared admission decision and the harness evidence that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindObservation {
    pub facts: ObservedBindFacts,
    pub admission: Admission,
    pub health: BindHealth,
}

struct AdapterCore {
    kind: Harness,
    tmux: Arc<dyn TmuxPort>,
}

impl AdapterCore {
    fn new(kind: Harness, tmux: Arc<dyn TmuxPort>) -> Self {
        Self { kind, tmux }
    }

    async fn capture(&self, pane: &str) -> Result<String> {
        self.tmux.capture(pane, CAPTURE_LINES).await
    }

    async fn discover_session(&self, pane: &str) -> Result<Option<String>> {
        Ok(session_id(&self.capture(pane).await?))
    }

    async fn readiness(&self, pane: &str) -> Result<Readiness> {
        let text = self.capture(pane).await?;
        Ok(classify_readiness(self.kind, &text))
    }

    async fn busy(&self, pane: &str) -> Result<bool> {
        Ok(is_busy(&self.capture(pane).await?))
    }

    async fn idle(&self, pane: &str) -> Result<bool> {
        Ok(shows_idle_prompt(self.kind, &self.capture(pane).await?))
    }

    async fn observe_bind(
        &self,
        descriptor: &SeatDescriptor,
        native_session_id: Option<String>,
        subagent_of: Option<String>,
    ) -> Result<BindObservation> {
        self.observe_bind_with_model_lookup(
            descriptor,
            native_session_id,
            subagent_of,
            crate::models::resolve_omp_model,
        )
        .await
    }

    async fn observe_bind_with_model_lookup<F>(
        &self,
        descriptor: &SeatDescriptor,
        native_session_id: Option<String>,
        subagent_of: Option<String>,
        resolve_model: F,
    ) -> Result<BindObservation>
    where
        F: FnOnce(&str) -> Result<String> + Send,
    {
        let Some(pane) = descriptor.pane.as_deref() else {
            return Ok(refused_observation(
                descriptor,
                subagent_of,
                ObservedBindFacts {
                    native_session_id,
                    pane: None,
                    model: None,
                    cwd: None,
                    readiness: Readiness::NotYet {
                        observed: "descriptor has no pane to inspect".to_string(),
                    },
                },
                false,
            ));
        };

        let text = self.capture(pane).await?;
        let readiness = classify_readiness(self.kind, &text);
        let mut model = observed_model(self.kind, &text);
        if self.kind == Harness::Omp
            && native_session_id.is_some()
            && let Some(observed) = model.as_deref()
        {
            model = Some(resolve_model(observed)?);
        }
        let facts = ObservedBindFacts {
            native_session_id,
            pane: Some(pane.to_string()),
            model,
            cwd: observed_cwd(self.kind, &text),
            readiness: readiness.clone(),
        };
        let facts_match = descriptor.harness == self.kind
            && matches!(readiness, Readiness::Ready)
            && descriptor.proc.is_some()
            && facts.native_session_id.is_some()
            && facts.cwd.as_deref() == Some(descriptor.folder.as_str())
            && descriptor.model.as_ref().is_none_or(|expected| {
                facts
                    .model
                    .as_ref()
                    .is_some_and(|actual| actual == expected)
            });
        Ok(refused_observation(
            descriptor,
            subagent_of,
            facts,
            facts_match,
        ))
    }

    async fn bind(&self, descriptor: &SeatDescriptor) -> Result<BindHealth> {
        Ok(self.observe_bind(descriptor, None, None).await?.health)
    }

    fn models(&self) -> Result<Vec<ModelRow>> {
        crate::models::models_for_harness(self.kind)
    }
}

fn refused_observation(
    descriptor: &SeatDescriptor,
    subagent_of: Option<String>,
    facts: ObservedBindFacts,
    bind_evidence: bool,
) -> BindObservation {
    let candidate = Candidate {
        id: descriptor.id.to_string(),
        harness: Some(descriptor.harness.to_string()),
        pane: facts.pane.clone(),
        folder: facts.cwd.clone(),
        bind_evidence,
        subagent_of,
    };
    let admission = admit(&candidate);
    let health = match (&admission, descriptor.proc) {
        (Admission::Admit, Some(proc)) => BindHealth::Bound { proc },
        (Admission::Admit, None) => BindHealth::Unbound {
            evidence: "admission passed but descriptor has no process identity".to_string(),
        },
        (Admission::Refuse(_), _) if facts.native_session_id.is_none() => BindHealth::Unbound {
            evidence: "no caller-supplied native session id — transcript discovery, planned session id, or phonehome evidence must resolve before binding".to_string(),
        },
        (Admission::Refuse(reason), _) => BindHealth::Unbound {
            evidence: reason.to_string(),
        },
    };
    BindObservation {
        facts,
        admission,
        health,
    }
}

fn is_busy(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("esc to interrupt")
        || lower.contains("esc interrupt")
        || lower.contains("esc cancel")
        || lower.contains(" working (")
        || lower.contains("◎ working")
        || text.lines().any(|line| {
            let trimmed = line.trim();
            (trimmed.starts_with('↓') && trimmed.contains("token"))
                || trimmed.contains("⟦esc⟧")
                || is_spinner_row(trimmed)
        })
}

/// Claude's in-turn spinner row, by shape: a glyph, one or more words, `…`,
/// then a parenthesised elapsed time — `✻ Fiddle-faddling… (11s · ↓ 120 tokens)`,
/// `✢ Compacting conversation… (3s)`. Glyph and verb both rotate, so neither
/// is listed; separators may be a space or NBSP. A finished turn's row
/// (`✻ Brewed for 11s · done`) has no ellipsis and does not match.
fn is_spinner_row(line: &str) -> bool {
    let is_gap = |c: char| c == ' ' || c == '\u{a0}';
    let mut chars = line.chars();
    let Some(glyph) = chars.next() else {
        return false;
    };
    if glyph.is_alphanumeric() || glyph.is_whitespace() {
        return false;
    }
    let Some((words, tail)) = chars
        .as_str()
        .strip_prefix(is_gap)
        .and_then(|rest| rest.split_once('…'))
    else {
        return false;
    };
    let is_word = |word: &str| {
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
    };
    if !words.split(is_gap).all(is_word) {
        return false;
    }
    let Some(elapsed) = tail
        .strip_prefix(is_gap)
        .and_then(|rest| rest.strip_prefix('('))
    else {
        return false;
    };
    let digits = elapsed.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && matches!(elapsed[digits..].chars().next(), Some('s' | 'm' | 'h'))
}

/// POSITIVE evidence that the harness sits idle at its prompt (plan 157 re-review
/// 3). This is NOT `!is_busy`: an empty, blank, truncated or dialog frame is not
/// evidence of anything. It is true only for Claude, and only when the footer
/// marks the prompt ready (so nothing busy shows, spinner row included) and the
/// composer sits between its two rules. Every other harness answers `false`
/// because none has a vetted idle frame.
///
/// Known limit, from live Claude 2.1.284 frames: while a reply streams text,
/// Claude shows no spinner, so that frame reads as idle. Callers that act on
/// this must rule that case out some other way.
fn shows_idle_prompt(kind: Harness, text: &str) -> bool {
    kind == Harness::Claude
        && matches!(classify_readiness(kind, text), Readiness::Ready)
        && has_claude_composer(text)
}

/// Claude's composer: a `❯` row between two rules. The row can carry
/// placeholder text (`❯ Try "fix typecheck errors"`). A dialog's `❯ 1. Yes`
/// never sits between rules.
fn has_claude_composer(text: &str) -> bool {
    let lines: Vec<_> = text.lines().collect();
    lines.windows(3).any(|window| {
        is_horizontal_rule(window[0])
            && window[1].trim_start().starts_with('❯')
            && is_horizontal_rule(window[2])
    })
}

fn classify_readiness(kind: Harness, text: &str) -> Readiness {
    if is_busy(text) {
        return Readiness::Busy;
    }
    let ready = match kind {
        Harness::Claude => {
            text.contains("bypass permissions on")
                || text.contains("shift+tab to cycle")
                || text.contains("auto mode on")
        }
        Harness::Copilot => text
            .lines()
            .any(|line| line.contains("? help") && line.contains("tab next tab")),
        Harness::Codex => text.lines().any(is_codex_footer),
        Harness::Pi => {
            has_rule_prompt(text)
                && text
                    .lines()
                    .any(|line| line.contains("◫") && line.contains("⬢"))
        }
        Harness::Omp => has_rule_prompt(text) && text.lines().any(|line| line.contains("◫")),
    };
    if ready {
        Readiness::Ready
    } else {
        Readiness::NotYet {
            observed: last_non_empty_line(text)
                .unwrap_or("empty pane capture")
                .to_string(),
        }
    }
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= 8 && trimmed.chars().all(|ch| ch == '─')
}

fn has_rule_prompt(text: &str) -> bool {
    let lines: Vec<_> = text.lines().collect();
    lines.windows(3).any(|window| {
        is_horizontal_rule(window[0]) && window[1].trim() == "❯" && is_horizontal_rule(window[2])
    })
}

fn last_non_empty_line(text: &str) -> Option<&str> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

fn session_id(text: &str) -> Option<String> {
    const LABELS: [&str; 4] = [
        "CLAUDE_CODE_SESSION_ID=",
        "COPILOT_AGENT_SESSION_ID=",
        "CODEX_THREAD_ID=",
        "PI_SESSION_ID=",
    ];
    text.lines().find_map(|line| {
        LABELS.iter().find_map(|label| {
            line.split_once(label).and_then(|(_, value)| {
                let id = value.split_whitespace().next()?.trim_matches(['\'', '"']);
                (!id.is_empty()).then(|| id.to_string())
            })
        })
    })
}

fn observed_model(kind: Harness, text: &str) -> Option<String> {
    match kind {
        Harness::Claude => text.lines().rev().find_map(|line| {
            line.split('•')
                .map(str::trim)
                .find(|field| {
                    field.contains("Opus") || field.contains("Sonnet") || field.contains("Haiku")
                })
                .map(str::to_string)
        }),
        Harness::Copilot => text.lines().rev().find_map(|line| {
            let (_, suffix) = line.split_once("tab next tab")?;
            let model = suffix.trim();
            (!model.is_empty()).then(|| model.to_string())
        }),
        Harness::Codex => text.lines().rev().find_map(|line| {
            is_codex_footer(line)
                .then(|| line.rsplit_once('·').map(|(left, _)| left.trim()))
                .flatten()
                .and_then(|left| left.split_whitespace().next())
                .map(str::to_string)
        }),
        Harness::Pi | Harness::Omp => {
            // The last model field is authoritative, including explicit absence.
            // Never fall back to a stale model quoted earlier in the pane.
            let (_, value) = text.lines().rev().find_map(|line| line.split_once('⬢'))?;
            let model = value.split(['·', '>']).next()?.trim();
            (!model.is_empty() && !(kind == Harness::Omp && model == "no-model"))
                .then(|| model.to_string())
        }
    }
}

fn observed_cwd(kind: Harness, text: &str) -> Option<String> {
    match kind {
        Harness::Codex => text.lines().rev().find_map(|line| {
            is_codex_footer(line)
                .then(|| {
                    line.rsplit_once('·')
                        .map(|(_, path)| expand_home(path.trim()))
                })
                .flatten()
        }),
        _ => text.lines().find_map(|line| {
            line.split_whitespace()
                .find(|word| word.starts_with('/') || word.starts_with("~/"))
                .map(|word| expand_home(word.trim_matches(['[', ']', '(', ')'])))
        }),
    }
}

fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return format!("{}/{rest}", home.to_string_lossy().trim_end_matches('/'));
    }
    path.to_string()
}

#[async_trait]
trait BindObserver: Send + Sync {
    async fn observe_bind(
        &self,
        descriptor: &SeatDescriptor,
        native_session_id: Option<String>,
        subagent_of: Option<String>,
    ) -> Result<BindObservation>;
}

macro_rules! adapter {
    ($name:ident, $kind:ident) => {
        pub struct $name {
            core: AdapterCore,
        }

        impl $name {
            pub fn new(tmux: Arc<dyn TmuxPort>) -> Self {
                Self {
                    core: AdapterCore::new(Harness::$kind, tmux),
                }
            }

            pub async fn observe_bind(
                &self,
                descriptor: &SeatDescriptor,
                native_session_id: Option<String>,
                subagent_of: Option<String>,
            ) -> Result<BindObservation> {
                self.core
                    .observe_bind(descriptor, native_session_id, subagent_of)
                    .await
            }
        }

        #[async_trait]
        impl BindObserver for $name {
            async fn observe_bind(
                &self,
                descriptor: &SeatDescriptor,
                native_session_id: Option<String>,
                subagent_of: Option<String>,
            ) -> Result<BindObservation> {
                self.core
                    .observe_bind(descriptor, native_session_id, subagent_of)
                    .await
            }
        }

        #[async_trait]
        impl HarnessPort for $name {
            fn kind(&self) -> Harness {
                Harness::$kind
            }

            async fn discover_session(&self, pane: &str) -> Result<Option<String>> {
                self.core.discover_session(pane).await
            }

            async fn bind(&self, descriptor: &SeatDescriptor) -> Result<BindHealth> {
                self.core.bind(descriptor).await
            }

            async fn readiness(&self, pane: &str) -> Result<Readiness> {
                self.core.readiness(pane).await
            }

            async fn busy(&self, pane: &str) -> Result<bool> {
                self.core.busy(pane).await
            }

            async fn idle(&self, pane: &str) -> Result<bool> {
                self.core.idle(pane).await
            }

            async fn models(&self) -> Result<Vec<ModelRow>> {
                self.core.models()
            }
        }
    };
}

adapter!(ClaudeHarness, Claude);
adapter!(CopilotHarness, Copilot);
adapter!(CodexHarness, Codex);
adapter!(PiHarness, Pi);
adapter!(OmpHarness, Omp);

/// Five distinct adapters, addressed by the frozen [`Harness`] enum.
///
/// This is deliberately a registry, not a composite `HarnessPort`: a composite
/// cannot return one truthful value from `kind()`, and Pi/Omp have separate
/// artifacts and readiness anchors.
///
/// # Composition recipe
///
/// In `crates/daemon/src/lib.rs`, use the following replacement for the
/// wave-0 single-fake shape (the PM owns this composition-root edit):
///
/// ```ignore
/// use pij_harnesses::HarnessRegistry;
///
/// pub harnesses: HarnessRegistry,
///
/// let harnesses = match config.adapters.harness {
///     AdapterChoice::Fake => HarnessRegistry::new([
///         Arc::new(FakeHarness::new(Harness::Claude)) as Arc<dyn HarnessPort>,
///         Arc::new(FakeHarness::new(Harness::Copilot)),
///         Arc::new(FakeHarness::new(Harness::Codex)),
///         Arc::new(FakeHarness::new(Harness::Pi)),
///         Arc::new(FakeHarness::new(Harness::Omp)),
///     ])?,
///     AdapterChoice::Real => HarnessRegistry::real(tmux.clone()),
/// };
/// ```
///
/// COMPOSED (wave 2). The refusal it replaced no longer exists. No config field is
/// required. Callers select ports with `get`, and real binding evidence with
/// `observe_bind`; the latter accepts the caller's authoritative subagent scope.
pub struct HarnessRegistry {
    adapters: BTreeMap<Harness, Arc<dyn HarnessPort>>,
    observers: BTreeMap<Harness, Arc<dyn BindObserver>>,
}

impl HarnessRegistry {
    pub fn new(adapters: impl IntoIterator<Item = Arc<dyn HarnessPort>>) -> Result<Self> {
        let mut by_kind = BTreeMap::new();
        for adapter in adapters {
            let kind = adapter.kind();
            if by_kind.insert(kind, adapter).is_some() {
                return Err(PijError::Adapter {
                    adapter: "harness registry".to_string(),
                    message: format!("duplicate adapter for {kind} — keep exactly one per harness"),
                });
            }
        }
        for kind in [
            Harness::Claude,
            Harness::Copilot,
            Harness::Codex,
            Harness::Pi,
            Harness::Omp,
        ] {
            if !by_kind.contains_key(&kind) {
                return Err(PijError::Adapter {
                    adapter: "harness registry".to_string(),
                    message: format!(
                        "missing adapter for {kind} — register all five harness variants"
                    ),
                });
            }
        }
        Ok(Self {
            adapters: by_kind,
            observers: BTreeMap::new(),
        })
    }

    pub fn real(tmux: Arc<dyn TmuxPort>) -> Self {
        let claude = Arc::new(ClaudeHarness::new(tmux.clone()));
        let copilot = Arc::new(CopilotHarness::new(tmux.clone()));
        let codex = Arc::new(CodexHarness::new(tmux.clone()));
        let pi = Arc::new(PiHarness::new(tmux.clone()));
        let omp = Arc::new(OmpHarness::new(tmux));
        let adapters = [
            claude.clone() as Arc<dyn HarnessPort>,
            copilot.clone(),
            codex.clone(),
            pi.clone(),
            omp.clone(),
        ];
        let observers = BTreeMap::from([
            (Harness::Claude, claude as Arc<dyn BindObserver>),
            (Harness::Copilot, copilot as Arc<dyn BindObserver>),
            (Harness::Codex, codex as Arc<dyn BindObserver>),
            (Harness::Pi, pi as Arc<dyn BindObserver>),
            (Harness::Omp, omp as Arc<dyn BindObserver>),
        ]);
        let mut registry = Self::new(adapters)
            .expect("the built-in registry contains exactly one adapter per Harness variant");
        registry.observers = observers;
        registry
    }

    pub fn get(&self, kind: Harness) -> &Arc<dyn HarnessPort> {
        self.adapters
            .get(&kind)
            .expect("HarnessRegistry construction proves every variant is present")
    }

    /// Observe real harness facts and run the one shared admission decision.
    /// `native_session_id` is caller knowledge: transcript discovery for
    /// Claude/Codex, the planned id for Copilot, and phonehome for Pi/Omp.
    /// `subagent_of` comes from the caller's process/session scope; inherited
    /// `PIJ_*` claims are never accepted as a substitute for either fact.
    pub async fn observe_bind(
        &self,
        descriptor: &SeatDescriptor,
        native_session_id: Option<String>,
        subagent_of: Option<String>,
    ) -> Result<BindObservation> {
        let observer = self
            .observers
            .get(&descriptor.harness)
            .ok_or_else(|| PijError::Adapter {
                adapter: "harness registry".to_string(),
                message: format!(
                    "{} uses a fake adapter with no real bind observer — script HarnessPort::bind in the testkit fake",
                    descriptor.harness
                ),
            })?;
        observer
            .observe_bind(descriptor, native_session_id, subagent_of)
            .await
    }

    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pij_core::admission::{Admission, RefusalReason};
    use pij_core::model::{Harness, ProcIdentity, Readiness, SeatDescriptor};
    use pij_core::ports::HarnessPort;
    use pij_testkit::block_on;
    use pij_testkit::fakes::FakeTmux;

    use super::{
        ClaudeHarness, CodexHarness, CopilotHarness, HarnessRegistry, OmpHarness, PiHarness,
        expand_home, shows_idle_prompt,
    };

    const CLAUDE: &str = include_str!("../../testkit/fixtures/harnesses/claude-ready.txt");
    const COPILOT: &str = include_str!("../../testkit/fixtures/harnesses/copilot-ready.txt");
    const CODEX: &str =
        include_str!("../../testkit/fixtures/harnesses/codex-rotated-placeholder-ready.txt");
    const CODEX_PLACEHOLDER: &str =
        include_str!("../../testkit/fixtures/harnesses/codex-placeholder-only.txt");
    const PI: &str = include_str!("../../testkit/fixtures/harnesses/pi-pane-ready.txt");
    const PI_WORKING: &str = include_str!("../../testkit/fixtures/harnesses/pi-pane-working.txt");
    const OMP: &str = include_str!("../../testkit/fixtures/harnesses/omp-pane-ready.txt");
    const OMP_WORKING: &str = include_str!("../../testkit/fixtures/harnesses/omp-working.txt");
    const CLAUDE_2_1_284_IDLE: &str =
        include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-idle.txt");
    const CLAUDE_2_1_284_AFTER_ESC: &str =
        include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-after-esc.txt");
    const CLAUDE_2_1_284_TOOL_RUN: &str =
        include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-tool-run.txt");
    const CLAUDE_2_1_284_TOOL_RUN_NARROW: &str =
        include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-tool-run-narrow.txt");

    fn adapter_with_captures(kind: Harness, fixture: &str) -> Arc<dyn HarnessPort> {
        let tmux = Arc::new(
            FakeTmux::new()
                .script_capture(fixture)
                .script_capture(fixture)
                .script_capture(fixture)
                .script_capture(fixture),
        );
        match kind {
            Harness::Claude => Arc::new(ClaudeHarness::new(tmux)),
            Harness::Copilot => Arc::new(CopilotHarness::new(tmux)),
            Harness::Codex => Arc::new(CodexHarness::new(tmux)),
            Harness::Pi => Arc::new(PiHarness::new(tmux)),
            Harness::Omp => Arc::new(OmpHarness::new(tmux)),
        }
    }

    #[test]
    fn every_harness_port_operation_runs_against_each_recorded_pane() {
        let cases = [
            (Harness::Claude, CLAUDE, "Opus 4.8", Readiness::Ready),
            (Harness::Copilot, COPILOT, "GPT-5.5", Readiness::Ready),
            (Harness::Codex, CODEX, "gpt-5.5", Readiness::Ready),
            (
                Harness::Pi,
                PI,
                "GPT-5.6 Sol Fast (Internal only)",
                Readiness::Ready,
            ),
            (Harness::Omp, OMP, "", Readiness::Ready),
        ];
        for (kind, fixture, model, expected) in cases {
            let adapter = adapter_with_captures(kind, fixture);
            assert_eq!(adapter.kind(), kind);
            assert_eq!(
                block_on(adapter.discover_session("%108")).expect("session discovery"),
                None,
                "real pane bytes do not invent a native session id for {kind}"
            );
            assert_eq!(
                block_on(adapter.readiness("%108")).expect("readiness"),
                expected
            );
            assert_eq!(
                block_on(adapter.busy("%108")).expect("busy"),
                expected == Readiness::Busy
            );
            match block_on(adapter.bind(&descriptor(kind, model))).expect("bind") {
                pij_core::model::BindHealth::Unbound { evidence } => {
                    assert!(evidence.contains("no caller-supplied native session id"));
                }
                other => panic!("trait bind cannot invent caller evidence: {other:?}"),
            }
            let models = block_on(adapter.models());
            if let Ok(rows) = models {
                assert!(!rows.is_empty(), "healthy {kind} catalog cannot be empty");
            }
        }
    }
    // AMBIENT-ENV GREEN-THAT-LIES, caught by CI's first Linux run: the copilot
    // footer fixture says `~/pi-hacking/pij`, and `expand_home` expands it against
    // the HOST's $HOME. Hard-coding this author's path made the comparison agree on
    // every machine the test had ever run on — because every machine was that one.
    // Derive it the same way the code under test does, so both sides agree on any
    // host. The mismatch arm stays covered by `bind_evidence_depends_on...`.
    fn cwd() -> String {
        expand_home("~/pi-hacking/pij")
    }

    fn descriptor(kind: Harness, model: &str) -> SeatDescriptor {
        let mut descriptor = SeatDescriptor::new("pij-harness-fixture", kind, cwd());
        descriptor.pane = Some("%108".to_string());
        descriptor.proc = Some(ProcIdentity {
            pid: 10_108,
            proc_start: 20_260_829_120_000,
        });
        descriptor.model = Some(model.to_string());
        descriptor
    }

    #[test]
    fn every_harness_variant_classifies_its_recorded_fixture() {
        let cases: [(Harness, &str, Arc<dyn HarnessPort>); 5] = [
            (
                Harness::Claude,
                CLAUDE,
                Arc::new(ClaudeHarness::new(Arc::new(
                    FakeTmux::new().script_capture(CLAUDE),
                ))),
            ),
            (
                Harness::Copilot,
                COPILOT,
                Arc::new(CopilotHarness::new(Arc::new(
                    FakeTmux::new().script_capture(COPILOT),
                ))),
            ),
            (
                Harness::Codex,
                CODEX,
                Arc::new(CodexHarness::new(Arc::new(
                    FakeTmux::new().script_capture(CODEX),
                ))),
            ),
            (
                Harness::Pi,
                PI,
                Arc::new(PiHarness::new(Arc::new(FakeTmux::new().script_capture(PI)))),
            ),
            (
                Harness::Omp,
                OMP,
                Arc::new(OmpHarness::new(Arc::new(
                    FakeTmux::new().script_capture(OMP),
                ))),
            ),
        ];

        for (kind, _fixture, adapter) in cases {
            assert_eq!(adapter.kind(), kind);
            assert_eq!(
                block_on(adapter.readiness("%108")).expect("fixture readiness"),
                Readiness::Ready,
                "{kind} fixture must reach its own idle anchor"
            );
        }
    }

    #[test]
    fn pi_status_footer_is_not_ready_while_the_spinner_is_active() {
        let adapter = PiHarness::new(Arc::new(FakeTmux::new().script_capture(PI_WORKING)));
        assert_eq!(
            block_on(adapter.readiness("%108")).expect("working fixture readiness"),
            Readiness::Busy,
            "the ready and working panes both contain a status footer; the active spinner wins"
        );
    }

    #[test]
    fn omp_status_footer_is_not_ready_while_the_spinner_is_active() {
        let adapter = OmpHarness::new(Arc::new(FakeTmux::new().script_capture(OMP_WORKING)));
        assert_eq!(
            block_on(adapter.readiness("%108")).expect("working fixture readiness"),
            Readiness::Busy,
            "the ready and working OMP panes share chrome; only the spinner distinguishes work"
        );
    }

    #[test]
    fn codex_rotating_placeholder_is_not_a_readiness_anchor() {
        let placeholder =
            CodexHarness::new(Arc::new(FakeTmux::new().script_capture(CODEX_PLACEHOLDER)));
        match block_on(placeholder.readiness("%108")).expect("placeholder classification") {
            Readiness::NotYet { observed } => {
                assert!(
                    observed.contains("Use /skills"),
                    "actual observation is retained"
                )
            }
            other => panic!("rotating placeholder must remain NotYet, got {other:?}"),
        }

        let rotated = CodexHarness::new(Arc::new(FakeTmux::new().script_capture(CODEX)));
        assert_eq!(
            block_on(rotated.readiness("%108")).expect("stable footer classification"),
            Readiness::Ready,
            "the persistent model/effort/cwd footer, not placeholder text, is the anchor"
        );
    }

    #[test]
    fn busy_guard_wins_over_a_visible_ready_anchor() {
        let pane = format!("{CLAUDE}\n↓ 42 tokens · esc to interrupt");
        let adapter = ClaudeHarness::new(Arc::new(FakeTmux::new().script_capture(pane)));
        assert_eq!(
            block_on(adapter.readiness("%108")).expect("busy classification"),
            Readiness::Busy
        );
    }

    /// Plan 157 re-review 3: only a vetted idle frame is idle evidence. Frames
    /// are live Claude Code 2.1.284 captures.
    #[test]
    fn only_a_vetted_idle_claude_frame_is_idle_evidence() {
        let idle = [
            (
                "live idle",
                include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-idle.txt"),
            ),
            (
                "live after Esc",
                include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-after-esc.txt"),
            ),
        ];
        let not_idle = [
            ("empty", ""),
            ("blank", "\n\n\n\n"),
            (
                "tool run",
                include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-tool-run.txt"),
            ),
            (
                "truncated spinner",
                include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-tool-run-narrow.txt"),
            ),
            (
                "permission dialog",
                include_str!(
                    "../../testkit/fixtures/harnesses/claude-2.1.284-permission-dialog.txt"
                ),
            ),
            (
                "manual mode, no ready footer",
                include_str!(
                    "../../testkit/fixtures/harnesses/claude-2.1.284-after-esc-manual-mode.txt"
                ),
            ),
        ];
        for (name, frame) in idle {
            assert!(shows_idle_prompt(Harness::Claude, frame), "{name}");
        }
        for (name, frame) in not_idle {
            assert!(!shows_idle_prompt(Harness::Claude, frame), "{name}");
        }
        // A footer without the composer between its rules (here a two-line
        // draft) is not the idle prompt.
        let live_idle = include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-idle.txt");
        let draft: String = live_idle
            .lines()
            .map(|line| {
                if line.starts_with('❯') {
                    "❯ first line\n  second line\n".to_string()
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        assert!(
            draft.contains("second line"),
            "the fixture has the composer row"
        );
        assert!(
            !shows_idle_prompt(Harness::Claude, &draft),
            "multi-line draft"
        );
        // The composer needs its rule ABOVE as well as below.
        let lines: Vec<&str> = live_idle.lines().collect();
        let composer = lines
            .iter()
            .position(|line| line.starts_with('❯'))
            .expect("the fixture has the composer row");
        let no_top_rule: String = lines
            .iter()
            .enumerate()
            .filter(|(index, _)| *index + 1 != composer)
            .map(|(_, line)| format!("{line}\n"))
            .collect();
        assert!(
            !shows_idle_prompt(Harness::Claude, &no_top_rule),
            "no top rule"
        );
        // No other harness has a vetted idle frame.
        assert!(!shows_idle_prompt(Harness::Omp, OMP));
        // The known limit: mid-stream text shows no spinner, so it reads idle.
        // The cold-wake guard rules this out by the age of `working`.
        assert!(shows_idle_prompt(
            Harness::Claude,
            include_str!("../../testkit/fixtures/harnesses/claude-2.1.284-streaming.txt")
        ));
    }

    #[test]
    fn claude_2_1_284_spinner_row_is_busy_at_any_width() {
        for (frame, pane) in [
            ("tool-run", CLAUDE_2_1_284_TOOL_RUN),
            ("tool-run-narrow", CLAUDE_2_1_284_TOOL_RUN_NARROW),
        ] {
            let adapter = adapter_with_captures(Harness::Claude, pane);
            assert!(
                block_on(adapter.busy("%108")).expect("busy"),
                "{frame}: a live spinner row is a turn in progress"
            );
            assert_eq!(
                block_on(adapter.readiness("%108")).expect("readiness"),
                Readiness::Busy,
                "{frame}: the spinner wins over the visible ready footer"
            );
        }
    }

    #[test]
    fn claude_2_1_284_idle_frames_and_finished_rows_are_not_busy() {
        for (frame, pane) in [
            ("idle", CLAUDE_2_1_284_IDLE),
            ("after-esc", CLAUDE_2_1_284_AFTER_ESC),
        ] {
            let adapter = adapter_with_captures(Harness::Claude, pane);
            assert!(!block_on(adapter.busy("%108")).expect("busy"), "{frame}");
            assert_eq!(
                block_on(adapter.readiness("%108")).expect("readiness"),
                Readiness::Ready,
                "{frame}"
            );
        }
        // Rows from the same live session once turns ended, and a tool's own
        // elapsed line: neither has the `…` + `(elapsed` shape.
        for line in [
            "✻ Brewed for 11s · done 11:32 pm",
            "✻ Worked for 2s · done 11:34 pm",
            "⏺ Sleeping for 40 seconds · 9s",
        ] {
            assert!(!super::is_busy(line), "{line}");
        }
    }

    #[test]
    fn spinner_row_variants_are_busy() {
        for (case, line) in [
            ("multi-word verb", "✻ Compacting conversation… (3s)"),
            (
                "NBSP after glyph",
                "✻\u{a0}Fiddle-faddling… (11s · ↓ 120 tokens)",
            ),
            ("NBSP between words", "✻ Compacting\u{a0}conversation… (3s)"),
            ("NBSP before elapsed", "✻ Fiddle-faddling…\u{a0}(12s)"),
            ("glyph ✢", "✢ Fiddle-faddling… (12s)"),
            ("glyph ✳", "✳ Fiddle-faddling… (12s)"),
            ("glyph *", "* Fiddle-faddling… (12s)"),
        ] {
            assert!(super::is_busy(line), "{case}: {line}");
        }
    }

    #[test]
    fn bind_observation_uses_shared_admission_and_refuses_subagents() {
        let adapter = ClaudeHarness::new(Arc::new(FakeTmux::new().script_capture(CLAUDE)));
        let observation = block_on(adapter.observe_bind(
            &descriptor(Harness::Claude, "Opus 4.8"),
            Some("claude-native-108".to_string()),
            Some("pij-parent".to_string()),
        ))
        .expect("observation");
        assert_eq!(
            observation.admission,
            Admission::Refuse(RefusalReason::Subagent {
                parent: "pij-parent".to_string()
            })
        );
        assert!(matches!(
            observation.health,
            pij_core::model::BindHealth::Unbound { .. }
        ));
    }

    #[test]
    fn bind_evidence_depends_on_observed_cwd_model_and_process() {
        let adapter = ClaudeHarness::new(Arc::new(FakeTmux::new().script_capture(CLAUDE)));
        let mut wrong = descriptor(Harness::Claude, "Opus 4.8");
        wrong.folder = "/some/other/worktree".to_string();
        let observation =
            block_on(adapter.observe_bind(&wrong, Some("claude-native-108".to_string()), None))
                .expect("observation");
        assert_eq!(
            observation.admission,
            Admission::Refuse(RefusalReason::NoBindEvidence),
            "claimed descriptor facts are not bind evidence; observed facts must agree"
        );
    }

    #[test]
    fn concrete_observation_returns_native_id_model_cwd_and_pane() {
        let adapter = CopilotHarness::new(Arc::new(FakeTmux::new().script_capture(COPILOT)));
        let observation = block_on(adapter.observe_bind(
            &descriptor(Harness::Copilot, "GPT-5.5"),
            Some("df4f1111-2222-4333-8444-555555555555".to_string()),
            None,
        ))
        .expect("observation");
        assert_eq!(observation.admission, Admission::Admit);
        assert_eq!(
            observation.facts.native_session_id.as_deref(),
            Some("df4f1111-2222-4333-8444-555555555555")
        );
        assert_eq!(observation.facts.model.as_deref(), Some("GPT-5.5"));
        assert_eq!(observation.facts.cwd.as_deref(), Some(cwd().as_str()));
        assert_eq!(observation.facts.pane.as_deref(), Some("%108"));
    }

    #[test]
    fn registry_enumerates_exactly_the_five_frozen_variants() {
        let tmux = Arc::new(FakeTmux::new());
        let registry = HarnessRegistry::real(tmux);
        assert_eq!(registry.len(), 5);
        for kind in [
            Harness::Claude,
            Harness::Copilot,
            Harness::Codex,
            Harness::Pi,
            Harness::Omp,
        ] {
            assert_eq!(registry.get(kind).kind(), kind);
        }
    }

    #[test]
    fn omp_footer_model_field_excludes_status_columns() {
        let footer = include_str!("../../testkit/fixtures/harnesses/omp-18-0-10-footer.txt");
        assert_eq!(
            super::observed_model(Harness::Omp, footer).as_deref(),
            Some("GPT-6 Astra (1M)"),
            "the model field ends before the effort/status columns"
        );
    }

    #[test]
    fn omp_model_bearing_footer_is_ready_without_weakening_busy_guard() {
        let footer = include_str!("../../testkit/fixtures/harnesses/omp-18-0-10-footer.txt");
        let pane = format!("────────\n❯\n────────\n◫ 13.3%/1M ⟲ · {}", footer.trim());
        assert_eq!(
            super::classify_readiness(Harness::Omp, &pane),
            Readiness::Ready
        );
        assert_eq!(
            super::classify_readiness(Harness::Omp, &format!("{pane}\n◎ working")),
            Readiness::Busy
        );
    }

    #[test]
    fn omp_bind_uses_catalog_evidence_instead_of_the_descriptor_claim() {
        let footer = include_str!("../../testkit/fixtures/harnesses/omp-18-0-10-footer.txt");
        let pane = format!(
            "{}\n────────\n❯\n────────\n◫ 13.3%/1M ⟲ · {}",
            cwd(),
            footer.trim()
        );
        for (requested, busy, admitted) in [
            ("github-copilot/gpt-6-astra-1m", false, true),
            ("local/not-the-observed-model", false, false),
            ("github-copilot/gpt-6-astra-1m", true, false),
        ] {
            let capture = if busy {
                format!("{pane}\n◎ working")
            } else {
                pane.clone()
            };
            let adapter = OmpHarness::new(Arc::new(FakeTmux::new().script_capture(capture)));
            let looked_up = std::sync::atomic::AtomicBool::new(false);
            let observation = block_on(adapter.core.observe_bind_with_model_lookup(
                &descriptor(Harness::Omp, requested),
                Some("omp-native-fixture".to_string()),
                None,
                |observed| {
                    looked_up.store(true, std::sync::atomic::Ordering::Relaxed);
                    assert_eq!(observed, "GPT-6 Astra (1M)");
                    Ok("github-copilot/gpt-6-astra-1m".to_string())
                },
            ))
            .expect("observed catalog identity");
            assert!(looked_up.load(std::sync::atomic::Ordering::Relaxed));
            assert_eq!(
                observation.facts.model.as_deref(),
                Some("github-copilot/gpt-6-astra-1m")
            );
            assert_eq!(observation.admission == Admission::Admit, admitted);
        }
    }

    #[test]
    fn omp_catalog_failure_never_becomes_a_successful_bind() {
        let footer = include_str!("../../testkit/fixtures/harnesses/omp-18-0-10-footer.txt");
        let pane = format!(
            "{}\n────────\n❯\n────────\n◫ 13.3%/1M ⟲ · {}",
            cwd(),
            footer.trim()
        );
        let adapter = OmpHarness::new(Arc::new(FakeTmux::new().script_capture(pane)));
        let result = block_on(adapter.core.observe_bind_with_model_lookup(
            &descriptor(Harness::Omp, "github-copilot/gpt-6-astra-1m"),
            Some("omp-native-fixture".to_string()),
            None,
            |_| {
                Err(pij_core::error::PijError::Adapter {
                    adapter: "fixture catalog".to_string(),
                    message: "unavailable".to_string(),
                })
            },
        ));
        assert!(
            result
                .expect_err("missing catalog is not model evidence")
                .to_string()
                .contains("unavailable")
        );
    }

    #[test]
    fn omp_18_1_11_no_model_footer_is_absence_not_a_requested_model_alias() {
        let absent =
            include_str!("../../testkit/fixtures/harnesses/omp-18-1-11-no-model-footer.txt");
        let earlier = include_str!("../../testkit/fixtures/harnesses/omp-18-0-10-footer.txt");
        assert!(super::observed_model(Harness::Omp, &format!("{earlier}\n{absent}")).is_none());
        let pane = format!("{}\n────────\n❯\n────────\n{}", cwd(), absent.trim());
        let adapter = OmpHarness::new(Arc::new(FakeTmux::new().script_capture(pane)));
        let observation = block_on(adapter.core.observe_bind_with_model_lookup(
            &descriptor(Harness::Omp, "github-copilot/gpt-6-astra-1m"),
            Some("omp-native-fixture".to_string()),
            None,
            |_| panic!("no-model is absence; never ask a catalog to turn it into a model"),
        ))
        .expect("observable model absence");
        assert!(observation.facts.model.is_none());
        assert_eq!(
            observation.admission,
            Admission::Refuse(RefusalReason::NoBindEvidence)
        );
    }
}

#[cfg(test)]
mod codex_0_153_4_footer {
    #![allow(clippy::doc_markdown)]
    use super::{Harness, Readiness, classify_readiness, observed_cwd, observed_model};

    const CAPTURE: &str = include_str!("../tests/fixtures/2026-09-08-codex-v0.153.4-pane-4409.txt");

    #[test]
    fn absolute_cwd_footer_reads_ready_model_and_cwd() {
        assert_eq!(
            classify_readiness(Harness::Codex, CAPTURE),
            Readiness::Ready
        );
        assert_eq!(
            observed_model(Harness::Codex, CAPTURE).as_deref(),
            Some("gpt-6-astra")
        );
        assert!(
            observed_cwd(Harness::Codex, CAPTURE)
                .is_some_and(|cwd| cwd.starts_with("/private/tmp/claude-501/")),
            "cwd comes from the right-hand side of the bar"
        );
    }

    #[test]
    fn home_relative_cwd_footer_still_reads() {
        let home = CAPTURE.replace("/private/tmp/claude-501/", "~/scratch/");
        assert_eq!(classify_readiness(Harness::Codex, &home), Readiness::Ready);
        assert_eq!(
            observed_model(Harness::Codex, &home).as_deref(),
            Some("gpt-6-astra")
        );
    }
}
