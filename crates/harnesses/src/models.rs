use std::fs;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use pij_core::error::{PijError, Result};
use pij_core::model::{Harness, ModelRow};
use serde_json::{Map, Value};

use crate::HarnessRegistry;

const OMP_TIMEOUT: Duration = Duration::from_secs(5);
const COPILOT_GPT56_LEVELS: [&str; 6] = ["none", "low", "medium", "high", "xhigh", "max"];
static COMMAND_ID: AtomicU64 = AtomicU64::new(0);

fn error(adapter: impl Into<String>, message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: adapter.into(),
        message: message.into(),
    }
}

fn required_string<'a>(row: &'a Map<String, Value>, field: &str) -> Result<&'a str> {
    row.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            error(
                "model catalog",
                format!("malformed row: missing non-empty `{field}` — refresh the model inventory"),
            )
        })
}

fn levels(row: &Map<String, Value>, field: &str) -> Result<Vec<String>> {
    let Some(value) = row.get(field) else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let values = value.as_array().ok_or_else(|| {
        error(
            "model catalog",
            format!(
                "malformed row: `{field}` must be an array or null — refresh the model inventory"
            ),
        )
    })?;
    values
        .iter()
        .map(|level| {
            level
                .as_str()
                .filter(|level| !level.trim().is_empty() && level.trim() == *level)
                .map(str::to_string)
                .ok_or_else(|| {
                    error(
                        "model catalog",
                        format!("malformed row: `{field}` contains an invalid level"),
                    )
                })
        })
        .collect()
}

/// Parse the committed TypeScript catalog output into the ruled Rust shape.
/// Missing, empty, and raw-null thinking levels all fold to an empty vector.
#[cfg(test)]
pub(crate) fn parse_catalog_json(input: &str) -> Result<Vec<ModelRow>> {
    let value: Value = serde_json::from_str(input).map_err(|cause| {
        error(
            "model catalog",
            format!("inventory is not valid JSON ({cause}) — refresh the model inventory"),
        )
    })?;
    let rows = value.as_array().ok_or_else(|| {
        error(
            "model catalog",
            "inventory must be a JSON array — refresh the model inventory",
        )
    })?;
    if rows.is_empty() {
        return Err(error(
            "model catalog",
            "inventory contained no models — verify the harness installation",
        ));
    }
    rows.iter()
        .map(|value| {
            let row = value.as_object().ok_or_else(|| {
                error(
                    "model catalog",
                    "malformed row: expected an object — refresh the model inventory",
                )
            })?;
            Ok(ModelRow {
                runtime: required_string(row, "runtime")?.to_string(),
                provider: required_string(row, "provider")?.to_string(),
                selector: required_string(row, "selector")?.to_string(),
                request_model_id: required_string(row, "requestModelId")?.to_string(),
                thinking_levels: levels(row, "levels")?,
            })
        })
        .collect()
}

fn home_file(parts: &[&str]) -> Result<String> {
    let home = std::env::var("HOME").map_err(|_| {
        error(
            "model catalog",
            "HOME is unset — set HOME so harness model registries can be found",
        )
    })?;
    let mut path = std::path::PathBuf::from(home);
    path.extend(parts);
    fs::read_to_string(&path).map_err(|cause| {
        error(
            "model catalog",
            format!(
                "cannot read {} ({cause}) — repair or reinstall the harness",
                path.display()
            ),
        )
    })
}

fn pi_rows(input: &str, runtime: Harness, only_github_copilot: bool) -> Result<Vec<ModelRow>> {
    let value: Value = serde_json::from_str(input).map_err(|cause| {
        error(
            format!("{runtime} model catalog"),
            format!("models.json is invalid ({cause}) — repair the harness model registry"),
        )
    })?;
    let providers = value
        .get("providers")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            error(
                format!("{runtime} model catalog"),
                "models.json has no providers object — repair the harness model registry",
            )
        })?;
    let mut output = Vec::new();
    for (provider, source) in providers {
        if only_github_copilot && provider != "github-copilot" {
            continue;
        }
        let source = source.as_object().ok_or_else(|| {
            error(
                format!("{runtime} model catalog"),
                format!("provider `{provider}` is malformed — repair the harness model registry"),
            )
        })?;
        let mut seen = std::collections::BTreeSet::new();
        if let Some(models) = source.get("models") {
            let models = models.as_array().ok_or_else(|| {
                error(
                    format!("{runtime} model catalog"),
                    format!("provider `{provider}` models must be an array"),
                )
            })?;
            for model in models {
                let model = model.as_object().ok_or_else(|| {
                    error(format!("{runtime} model catalog"), "malformed model row")
                })?;
                let id = required_string(model, "id")?;
                seen.insert(id.to_string());
                output.push(pi_row(runtime, provider, id, model)?);
            }
        }
        if let Some(overrides) = source.get("modelOverrides") {
            let overrides = overrides.as_object().ok_or_else(|| {
                error(
                    format!("{runtime} model catalog"),
                    format!("provider `{provider}` modelOverrides must be an object"),
                )
            })?;
            for (id, model) in overrides {
                if seen.contains(id) {
                    continue;
                }
                let model = model.as_object().ok_or_else(|| {
                    error(
                        format!("{runtime} model catalog"),
                        "malformed model override",
                    )
                })?;
                output.push(pi_row(runtime, provider, id, model)?);
            }
        }
    }
    if output.is_empty() {
        return Err(error(
            format!("{runtime} model catalog"),
            "inventory contained no models — sync the harness model registry",
        ));
    }
    Ok(output)
}

fn pi_row(
    runtime: Harness,
    provider: &str,
    id: &str,
    source: &Map<String, Value>,
) -> Result<ModelRow> {
    let thinking_levels = if provider == "github-copilot" && is_copilot_gpt56(id) {
        COPILOT_GPT56_LEVELS
            .iter()
            .map(|level| (*level).to_string())
            .collect()
    } else {
        source
            .get("thinkingLevelMap")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(level, _)| level.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    let selector = if runtime == Harness::Copilot {
        id.to_string()
    } else {
        format!("{provider}/{id}")
    };
    Ok(ModelRow {
        runtime: runtime.to_string(),
        provider: provider.to_string(),
        selector,
        request_model_id: id.to_string(),
        thinking_levels,
    })
}

fn is_copilot_gpt56(id: &str) -> bool {
    matches!(
        id.strip_suffix("-1m").unwrap_or(id),
        "gpt-5.6-sol" | "gpt-5.6-sol-fast" | "gpt-5.6-terra" | "gpt-5.6-luna"
    )
}

fn aliases(runtime: Harness, provider: &str, ids: &[&str]) -> Vec<ModelRow> {
    ids.iter()
        .map(|id| ModelRow {
            runtime: runtime.to_string(),
            provider: provider.to_string(),
            selector: (*id).to_string(),
            request_model_id: (*id).to_string(),
            thinking_levels: if runtime == Harness::Codex
                && (id.starts_with("gpt-5") || id.starts_with('o'))
            {
                let levels = if id.starts_with("gpt-5") {
                    &["minimal", "low", "medium", "high", "xhigh"][..]
                } else {
                    &["minimal", "low", "medium", "high"][..]
                };
                levels.iter().map(|level| (*level).to_string()).collect()
            } else if runtime == Harness::Copilot && is_copilot_gpt56(id) {
                COPILOT_GPT56_LEVELS
                    .iter()
                    .map(|level| (*level).to_string())
                    .collect()
            } else {
                Vec::new()
            },
        })
        .collect()
}

fn codex_rows() -> Vec<ModelRow> {
    let mut ids = Vec::new();
    if let Ok(config) = home_file(&[".codex", "config.toml"]) {
        for raw in config.lines() {
            let line = raw.trim();
            if line.starts_with('[') {
                break;
            }
            if let Some(value) = line
                .strip_prefix("model")
                .and_then(|rest| rest.trim().strip_prefix('='))
            {
                let id = value.trim().trim_matches(['\'', '"']);
                if !id.is_empty() {
                    ids.push(id.to_string());
                    break;
                }
            }
        }
    }
    for fallback in [
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.5",
        "o3",
    ] {
        if !ids.iter().any(|id| id == fallback) {
            ids.push(fallback.to_string());
        }
    }
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    aliases(Harness::Codex, "codex", &refs)
}

#[derive(Debug)]
struct CommandResult {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_omp_command() -> std::io::Result<CommandResult> {
    let id = COMMAND_ID.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("pij-models-{}-{id}", std::process::id()));
    let stdout_path = base.with_extension("out");
    let stderr_path = base.with_extension("err");
    let stdout = fs::File::create(&stdout_path)?;
    let stderr = fs::File::create(&stderr_path)?;
    let mut child = Command::new("omp")
        .args(["models", "--json", "--no-extensions"])
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= OMP_TIMEOUT {
            child.kill()?;
            let _ = child.wait();
            let _ = fs::remove_file(&stdout_path);
            let _ = fs::remove_file(&stderr_path);
            return Ok(CommandResult {
                status: None,
                stdout: String::new(),
                stderr: "timed out after 5 seconds".to_string(),
            });
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = fs::read_to_string(&stdout_path)?;
    let stderr = fs::read_to_string(&stderr_path)?;
    let _ = fs::remove_file(stdout_path);
    let _ = fs::remove_file(stderr_path);
    Ok(CommandResult {
        status: status.code(),
        stdout,
        stderr,
    })
}

fn omp_inventory_with(run: impl FnOnce() -> std::io::Result<CommandResult>) -> Result<Value> {
    let result = run().map_err(|cause| {
        error(
            "omp model catalog",
            format!(
                "cannot run `omp models --json --no-extensions` ({cause}) — install or repair OMP"
            ),
        )
    })?;
    if result.status != Some(0) {
        return Err(error(
            "omp model catalog",
            format!(
                "inventory command failed ({}) — run `omp models --json --no-extensions`",
                result.stderr.trim()
            ),
        ));
    }
    let value: Value = serde_json::from_str(&result.stdout).map_err(|cause| {
        error(
            "omp model catalog",
            format!("inventory is invalid JSON ({cause}) — repair OMP"),
        )
    })?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            error(
                "omp model catalog",
                "inventory has no models array — update OMP",
            )
        })?;
    if models.is_empty() {
        return Err(error(
            "omp model catalog",
            "inventory contained no models — update or reconfigure OMP",
        ));
    }
    Ok(value)
}

fn omp_rows_with(run: impl FnOnce() -> std::io::Result<CommandResult>) -> Result<Vec<ModelRow>> {
    let value = omp_inventory_with(run)?;
    let models = value["models"]
        .as_array()
        .expect("validated OMP model array");
    models
        .iter()
        .map(|value| {
            let source = value
                .as_object()
                .ok_or_else(|| error("omp model catalog", "malformed model row"))?;
            let provider = required_string(source, "provider")?;
            let id = required_string(source, "id")?;
            let selector = required_string(source, "selector")?;
            let request_model_id = if provider == "github-copilot" {
                id.strip_suffix("-1m").unwrap_or(id)
            } else {
                id
            };
            let mut thinking_levels = levels(source, "thinking")?;
            if provider == "github-copilot" && is_copilot_gpt56(request_model_id) {
                thinking_levels = COPILOT_GPT56_LEVELS
                    .iter()
                    .map(|level| (*level).to_string())
                    .collect();
            }
            Ok(ModelRow {
                runtime: Harness::Omp.to_string(),
                provider: provider.to_string(),
                selector: selector.to_string(),
                request_model_id: request_model_id.to_string(),
                thinking_levels,
            })
        })
        .collect()
}

/// Resolve an observed OMP label from the same inventory used by model discovery.
/// A claimed descriptor is deliberately not an input: it cannot disambiguate evidence.
pub(crate) fn resolve_omp_model(observed: &str) -> Result<String> {
    resolve_omp_model_with(observed, run_omp_command)
}

fn resolve_omp_model_with(
    observed: &str,
    run: impl FnOnce() -> std::io::Result<CommandResult>,
) -> Result<String> {
    let value = omp_inventory_with(run)?;
    let mut matched = None;
    for value in value["models"]
        .as_array()
        .expect("validated OMP model array")
    {
        let source = value
            .as_object()
            .ok_or_else(|| error("omp model catalog", "malformed model row"))?;
        required_string(source, "provider")?;
        let id = required_string(source, "id")?;
        let selector = required_string(source, "selector")?;
        let name = source.get("name").and_then(Value::as_str);
        if observed != selector && observed != id && name != Some(observed) {
            continue;
        }
        if let Some(previous) = matched
            && previous != selector
        {
            return Err(error(
                "omp model catalog",
                format!(
                    "observed model {observed:?} is ambiguous between {previous} and {selector}"
                ),
            ));
        }
        matched = Some(selector);
    }
    matched.map(str::to_string).ok_or_else(|| {
        error(
            "omp model catalog",
            format!("observed model {observed:?} is absent from the OMP inventory; refusing to infer its selector"),
        )
    })
}

pub(crate) fn models_for_harness(kind: Harness) -> Result<Vec<ModelRow>> {
    let rows = match kind {
        Harness::Pi => pi_rows(&home_file(&[".pi", "agent", "models.json"])?, kind, false)?,
        Harness::Copilot => {
            let mut rows = home_file(&[".pi", "agent", "models.json"])
                .ok()
                .and_then(|input| pi_rows(&input, kind, true).ok())
                .unwrap_or_default();
            let seen: std::collections::BTreeSet<_> = rows
                .iter()
                .map(|row| row.request_model_id.clone())
                .collect();
            rows.extend(
                aliases(
                    kind,
                    "github-copilot",
                    &[
                        "gpt-5.6-sol",
                        "gpt-5.6-sol-fast",
                        "gpt-5.6-terra",
                        "gpt-5.6-luna",
                    ],
                )
                .into_iter()
                .filter(|row| !seen.contains(&row.request_model_id)),
            );
            rows
        }
        Harness::Claude => aliases(
            kind,
            "claude",
            &[
                "claude-opus-5-5",
                "claude-sonnet-5-5",
                "claude-opus-5",
                "claude-fable-5",
                "claude-sonnet-5",
                "claude-opus-4-8",
                "claude-sonnet-4-6",
                "claude-haiku-4-5-20251001",
                "claude-opus-4-5",
                "claude-sonnet-4-5",
                "claude-haiku-4-5",
            ],
        ),
        Harness::Codex => codex_rows(),
        Harness::Omp => omp_rows_with(run_omp_command)?,
    };
    if rows.is_empty() {
        Err(error(
            format!("{kind} model catalog"),
            "inventory contained no models — repair the harness installation",
        ))
    } else {
        Ok(rows)
    }
}

/// Catalog assembly over the five harness adapters.
pub struct ModelCatalog<'a> {
    registry: &'a HarnessRegistry,
}

impl<'a> ModelCatalog<'a> {
    /// Construct a catalog from the composed harness registry.
    pub fn new(registry: &'a HarnessRegistry) -> Self {
        Self { registry }
    }

    /// List rows by exact runtime or provider. An unfiltered/provider query must
    /// assemble the whole inventory; a runtime query touches only that adapter.
    pub async fn list(&self, filter: Option<&str>) -> Result<Vec<ModelRow>> {
        if let Some(runtime) = filter.and_then(Harness::parse) {
            return self.runtime_rows(runtime).await;
        }
        let mut rows = Vec::new();
        for kind in [
            Harness::Claude,
            Harness::Copilot,
            Harness::Codex,
            Harness::Pi,
            Harness::Omp,
        ] {
            rows.extend(self.runtime_rows(kind).await?);
        }
        if let Some(provider) = filter {
            rows.retain(|row| row.provider == provider);
        }
        Ok(rows)
    }

    async fn runtime_rows(&self, runtime: Harness) -> Result<Vec<ModelRow>> {
        let rows = self.registry.get(runtime).models().await?;
        if rows.is_empty() {
            Err(error(
                format!("{runtime} model catalog"),
                "inventory contained no models — repair the harness installation",
            ))
        } else {
            Ok(rows)
        }
    }
}

/// # Composition recipe
///
/// The PM-owned composition root imports `pij_harnesses::ModelCatalog`, constructs
/// `ModelCatalog::new(&services.harnesses)`, and passes it to the CLI models handler.
/// No `AdapterChoice` arm or config field changes: the harness registry is already
/// composed, and runtime-filtered calls select its existing ports.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pij_core::model::Harness;
    use pij_core::ports::HarnessPort;
    use pij_testkit::block_on;
    use pij_testkit::fakes::{FakeHarness, FakeTmux};

    use super::{
        COPILOT_GPT56_LEVELS, CommandResult, ModelCatalog, omp_rows_with, parse_catalog_json,
        pi_rows,
    };
    use crate::{
        ClaudeHarness, CodexHarness, CopilotHarness, HarnessRegistry, OmpHarness, PiHarness,
    };

    const CATALOG: &str = include_str!("../../testkit/fixtures/cli/models.json");
    const MALFORMED: &str =
        include_str!("../../testkit/fixtures/malformed/ts/models-missing-request-id.json");

    #[test]
    fn corpus_preserves_runtime_provider_selector_and_request_id() {
        let rows = parse_catalog_json(CATALOG).expect("committed TS catalog");
        assert!(rows.iter().any(|row| {
            row.runtime == "pi"
                && row.provider == "github-copilot"
                && row.selector == "github-copilot/gpt-5.5"
                && row.request_model_id == "gpt-5.5"
        }));
        assert!(rows.iter().any(|row| {
            row.runtime == "claude" && row.provider == "claude" && row.selector == "claude-fable-5"
        }));
    }

    #[test]
    fn claude_catalog_offers_opus_5_5() {
        let rows = super::models_for_harness(Harness::Claude).expect("claude aliases");
        assert!(
            rows.iter().any(|row| row.selector == "claude-opus-5-5"
                && row.request_model_id == "claude-opus-5-5"),
            "Opus 5.5 must be discoverable for claude spawns"
        );
    }

    #[test]
    fn claude_catalog_offers_sonnet_5_5() {
        let rows = super::models_for_harness(Harness::Claude).expect("claude aliases");
        assert!(
            rows.iter().any(|row| row.selector == "claude-sonnet-5-5"
                && row.request_model_id == "claude-sonnet-5-5"),
            "Sonnet 5.5 must be discoverable for claude spawns"
        );
    }

    #[test]
    fn production_catalog_matches_the_committed_folded_golden() {
        let rows = parse_catalog_json(CATALOG).expect("committed TS catalog");
        let mut lines: Vec<String> = rows
            .iter()
            .map(|row| {
                let levels = if row.thinking_levels.is_empty() {
                    "<none>".to_string()
                } else {
                    row.thinking_levels.join("/")
                };
                format!(
                    "{}\t{}\t{}\t{}\t{}",
                    row.runtime, row.provider, row.selector, row.request_model_id, levels
                )
            })
            .collect();
        lines.sort();
        pij_testkit::golden::assert_golden(
            "models-summary.tsv",
            &format!("{}\n", lines.join("\n")),
        );
    }

    #[test]
    fn absent_empty_and_raw_null_levels_fold_but_missing_request_id_fails_by_name() {
        let rows = parse_catalog_json(CATALOG).expect("valid catalog");
        assert!(
            rows.iter()
                .find(|row| row.selector == "claude-fable-5")
                .unwrap()
                .thinking_levels
                .is_empty()
        );
        assert!(
            rows.iter()
                .find(|row| row.selector.contains("gpt-5.5"))
                .unwrap()
                .thinking_levels
                .is_empty()
        );
        let error =
            parse_catalog_json(MALFORMED).expect_err("missing requestModelId invalidates all rows");
        assert!(error.to_string().contains("requestModelId"), "{error}");
    }

    #[test]
    fn pi_and_copilot_parsers_keep_runtime_provider_and_selector_separate() {
        let source = r#"{"providers":{"github-copilot":{"models":[{"id":"gpt-5.6-sol-fast","thinkingLevelMap":{"high":"high"}}]},"sakana":{"models":[{"id":"fugu","thinkingLevelMap":{"high":"high","xhigh":"max"}}]}}}"#;
        let pi = pi_rows(source, Harness::Pi, false).expect("pi inventory");
        let fugu = pi
            .iter()
            .find(|row| row.request_model_id == "fugu")
            .unwrap();
        assert_eq!(fugu.runtime, "pi");
        assert_eq!(fugu.provider, "sakana");
        assert_eq!(fugu.selector, "sakana/fugu");
        assert_eq!(fugu.thinking_levels, ["high", "xhigh"]);

        let copilot = pi_rows(source, Harness::Copilot, true).expect("copilot projection");
        assert_eq!(copilot.len(), 1);
        assert_eq!(copilot[0].runtime, "copilot");
        assert_eq!(copilot[0].provider, "github-copilot");
        assert_eq!(copilot[0].selector, "gpt-5.6-sol-fast");
        assert_eq!(copilot[0].thinking_levels, COPILOT_GPT56_LEVELS);
    }

    #[test]
    fn omp_parser_accepts_null_levels_and_preserves_alias_request_id() {
        let rows = omp_rows_with(|| {
            Ok(CommandResult {
                status: Some(0),
                stdout: r#"{"models":[{"provider":"github-copilot","id":"gpt-5.6-sol-fast-1m","selector":"github-copilot/gpt-5.6-sol-fast-1m","thinking":null}]}"#.to_string(),
                stderr: String::new(),
            })
        })
        .expect("valid OMP inventory");
        assert_eq!(rows[0].runtime, "omp");
        assert_eq!(rows[0].selector, "github-copilot/gpt-5.6-sol-fast-1m");
        assert_eq!(rows[0].request_model_id, "gpt-5.6-sol-fast");
        assert_eq!(rows[0].thinking_levels, COPILOT_GPT56_LEVELS);
    }

    #[test]
    fn omp_parser_rejects_one_bad_row_without_returning_survivors() {
        let result = omp_rows_with(|| {
            Ok(CommandResult {
            status: Some(0),
            stdout: r#"{"models":[{"provider":"openrouter","id":"good","selector":"openrouter/good","thinking":null},{"provider":"openrouter","id":"bad"}]}"#.to_string(),
            stderr: String::new(),
        })
        });
        assert!(result.is_err());
    }

    #[test]
    fn runtime_filter_touches_only_its_adapter_but_provider_filter_requires_all() {
        let adapters: Vec<Arc<dyn HarnessPort>> = [
            Harness::Claude,
            Harness::Copilot,
            Harness::Codex,
            Harness::Pi,
            Harness::Omp,
        ]
        .into_iter()
        .map(|kind| {
            let fake = FakeHarness::new(kind);
            if kind == Harness::Codex {
                Arc::new(fake.with_models(vec![pij_core::model::ModelRow {
                    runtime: "codex".to_string(),
                    provider: "codex".to_string(),
                    selector: "gpt-5.6-sol".to_string(),
                    request_model_id: "gpt-5.6-sol".to_string(),
                    thinking_levels: vec!["high".to_string()],
                }])) as Arc<dyn HarnessPort>
            } else {
                Arc::new(fake) as Arc<dyn HarnessPort>
            }
        })
        .collect();
        let registry = HarnessRegistry::new(adapters).expect("complete registry");
        let catalog = ModelCatalog::new(&registry);
        assert_eq!(block_on(catalog.list(Some("codex"))).unwrap().len(), 1);
        assert!(block_on(catalog.list(Some("openrouter"))).is_err());
        assert!(block_on(catalog.list(None)).is_err());
    }

    #[test]
    fn every_real_adapter_returns_non_empty_models_or_an_honest_unavailable_error() {
        let tmux = Arc::new(FakeTmux::new());
        let adapters: [Arc<dyn HarnessPort>; 5] = [
            Arc::new(ClaudeHarness::new(tmux.clone())),
            Arc::new(CopilotHarness::new(tmux.clone())),
            Arc::new(CodexHarness::new(tmux.clone())),
            Arc::new(PiHarness::new(tmux.clone())),
            Arc::new(OmpHarness::new(tmux)),
        ];
        for adapter in adapters {
            match block_on(adapter.models()) {
                Ok(rows) => assert!(
                    !rows.is_empty(),
                    "{} claimed a healthy empty catalog",
                    adapter.kind()
                ),
                Err(error) => assert!(
                    !error.to_string().contains("u-models in wave 3"),
                    "placeholder survived: {error}"
                ),
            }
        }
    }

    #[test]
    fn omp_observed_names_resolve_through_catalog_without_model_specific_aliases() {
        let inventory = r#"{"models":[{"provider":"github-copilot","id":"gpt-6-astra-1m","selector":"github-copilot/gpt-6-astra-1m","name":"GPT-6 Astra (1M)"},{"provider":"local","id":"private-model","selector":"local/private-model","name":"Personal Model"}]}"#;
        for (observed, expected) in [
            ("GPT-6 Astra (1M)", "github-copilot/gpt-6-astra-1m"),
            ("gpt-6-astra-1m", "github-copilot/gpt-6-astra-1m"),
            (
                "github-copilot/gpt-6-astra-1m",
                "github-copilot/gpt-6-astra-1m",
            ),
            ("Personal Model", "local/private-model"),
        ] {
            let actual = super::resolve_omp_model_with(observed, || {
                Ok(CommandResult {
                    status: Some(0),
                    stdout: inventory.to_string(),
                    stderr: String::new(),
                })
            })
            .expect("unique observed catalog identity");
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn omp_observed_aliases_refuse_ambiguity_instead_of_believing_a_claim() {
        let result = super::resolve_omp_model_with("Shared Name", || {
            Ok(CommandResult {
            status: Some(0),
            stdout: r#"{"models":[{"provider":"one","id":"first","selector":"one/first","name":"Shared Name"},{"provider":"two","id":"second","selector":"two/second","name":"Shared Name"}]}"#.to_string(),
            stderr: String::new(),
        })
        });
        assert!(
            result
                .expect_err("ambiguous labels are not evidence")
                .to_string()
                .contains("ambiguous")
        );
    }

    #[test]
    fn omp_observed_aliases_reject_unknown_labels_and_partial_catalogs() {
        for (label, inventory) in [
            (
                "invented alias",
                r#"{"models":[{"provider":"local","id":"real","selector":"local/real","name":"Real Model"}]}"#,
            ),
            (
                "Real Model",
                r#"{"models":[{"provider":"local","id":"real","selector":"local/real","name":"Real Model"},{"provider":"broken"}]}"#,
            ),
        ] {
            assert!(
                super::resolve_omp_model_with(label, || Ok(CommandResult {
                    status: Some(0),
                    stdout: inventory.to_string(),
                    stderr: String::new(),
                }))
                .is_err()
            );
        }
    }
}
