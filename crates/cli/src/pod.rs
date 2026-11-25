//! `pij pod` answers as text (PRD 008): every warning with its fix first.

use serde_json::Value;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("-")
}

/// A `pod plan`, `up` or `show` answer as lines a person reads.
pub fn render(data: &Value) -> String {
    let mut lines = Vec::new();
    let pod = text(&data["pod"]);
    if let Some(template) = data["template"].as_object() {
        lines.push(format!(
            "pod {pod} from {} (version {}) in project {}",
            text(&template["source"]),
            text(&template["version"]),
            text(&data["project"])
        ));
    } else {
        lines.push(format!("pod {pod} in project {}", text(&data["project"])));
    }
    if let Some(warnings) = data["warnings"]
        .as_array()
        .filter(|found| !found.is_empty())
    {
        lines.push(format!("{} warning(s), none refuses:", warnings.len()));
        // plan and up answer {about, text, fix, line}; set, down, revive and
        // stage answer plain strings (#346 B346-R1-08).
        lines.extend(warnings.iter().map(|warning| {
            format!(
                "  ! {}",
                warning.as_str().unwrap_or_else(|| text(&warning["line"]))
            )
        }));
    }
    if let Some(slots) = data["slots"].as_array() {
        lines.push("slots:".to_string());
        for slot in slots {
            let tools: Vec<&str> = slot["tools"]
                .as_array()
                .map(|tools| tools.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            lines.push(format!(
                "  {}: role {}, {} {}{}",
                text(&slot["slot"]),
                text(&slot["role"]),
                text(&slot["harness"]),
                slot["model"].as_str().unwrap_or("(role names no model)"),
                if tools.is_empty() {
                    String::new()
                } else {
                    format!(", tools {}", tools.join(", "))
                }
            ));
            if let Some(turn) = slot["first_turn"].as_str() {
                lines.push("    first turn:".to_string());
                lines.extend(turn.lines().map(|line| format!("      {line}")));
            } else if let Some(why) = slot["first_turn_error"].as_str() {
                lines.push(format!("    first turn: does not build: {why}"));
            }
        }
    }
    if let Some(members) = data["members"].as_array() {
        lines.push("members:".to_string());
        for member in members {
            // down and revive answer what happened to each member.
            if let Some(outcome) = member["outcome"].as_str() {
                lines.push(format!(
                    "  {}: {}: {outcome}",
                    text(&member["slot"]),
                    text(&member["seat"])
                ));
                continue;
            }
            lines.push(format!(
                "  {}: {} in {}{} ({}), canary {}{}",
                text(&member["slot"]),
                text(&member["seat"]),
                text(&member["role"]),
                member["type"]
                    .as_str()
                    .map(|kind| format!(" [{kind}]"))
                    .unwrap_or_default(),
                member["state"].as_str().unwrap_or("started"),
                text(&member["canary"]),
                member["folder"]
                    .as_str()
                    .map(|folder| format!(", in {folder}"))
                    .unwrap_or_default()
            ));
        }
    }
    if let Some(consulted) = data["consulted"]
        .as_array()
        .filter(|found| !found.is_empty())
    {
        lines.push("consulted:".to_string());
        for role in consulted {
            let holders = match (&role["live_holder"], role["live_holders"].as_array()) {
                (Value::Bool(live), _) => if *live {
                    "a live holder"
                } else {
                    "no live holder"
                }
                .to_string(),
                (_, Some(seats)) if !seats.is_empty() => {
                    seats.iter().map(text).collect::<Vec<_>>().join(", ")
                }
                _ => "no live holder".to_string(),
            };
            lines.push(format!(
                "  {}{}: {holders}",
                text(&role["role"]),
                if role["before_up"] == true {
                    " (needed before up)"
                } else {
                    ""
                }
            ));
        }
    }
    if let Some(wiring) = data["wiring"].as_array().filter(|found| !found.is_empty()) {
        lines.push("wiring:".to_string());
        lines.extend(wiring.iter().map(|wire| {
            format!(
                "  {} -> {}{}",
                text(&wire["from"]),
                text(&wire["to"]),
                wire["what"]
                    .as_str()
                    .filter(|what| !what.is_empty())
                    .map(|what| format!(": {what}"))
                    .unwrap_or_default()
            )
        }));
    }
    if let Some(deliverables) = data["deliverables"]
        .as_array()
        .filter(|found| !found.is_empty())
    {
        lines.push("deliverables:".to_string());
        for deliverable in deliverables {
            lines.push(format!(
                "  {}: owner {}{}",
                text(&deliverable["id"]),
                text(&deliverable["owner"]),
                deliverable["write_back"]
                    .as_str()
                    .map(|role| format!(", written back to {role}"))
                    .unwrap_or_default()
            ));
        }
    }
    if let Some(degraded) = data["degraded"]
        .as_array()
        .filter(|found| !found.is_empty())
    {
        lines.push("degraded:".to_string());
        lines.extend(
            degraded
                .iter()
                .map(|slot| format!("  {}: {}", text(&slot["slot"]), text(&slot["canary"]))),
        );
    }
    if let Some(activity) = data["activity"].as_array() {
        lines.push(format!("{} event(s):", activity.len()));
        lines.extend(activity.iter().map(|line| {
            format!(
                "  {} [{}] {}",
                line["at"]
                    .as_u64()
                    .map_or("-".to_string(), |at| at.to_string()),
                text(&line["slot"]),
                text(&line["line"])
            )
        }));
        if let Some(transcripts) = data["transcripts"].as_array() {
            lines.push("transcripts:".to_string());
            lines.extend(transcripts.iter().map(|member| {
                format!(
                    "  {} {}: {}",
                    text(&member["slot"]),
                    text(&member["seat"]),
                    text(&member["transcript"])
                )
            }));
        }
    }
    if let Some(members) = data["sessions"].as_array() {
        for member in members {
            lines.push(format!(
                "{} {}: {} session(s)",
                text(&member["slot"]),
                text(&member["seat"]),
                member["total"].as_u64().unwrap_or(0)
            ));
            for session in member["sessions"].as_array().into_iter().flatten() {
                lines.push(
                    format!(
                        "  {} -> {}  {}  {} {}  session {}  context {}",
                        session["started_at"]
                            .as_u64()
                            .map_or("-".to_string(), |at| at.to_string()),
                        session["ended_at"]
                            .as_u64()
                            .map_or("running".to_string(), |at| at.to_string()),
                        text(&session["why"]),
                        text(&session["harness"]),
                        text(&session["model"]),
                        text(&session["session"]),
                        session["context_tokens"]
                    )
                    .replace('"', ""),
                );
            }
        }
    }
    for key in ["canary", "started", "writes"] {
        if let Some(line) = data[key].as_str() {
            lines.push(format!("{key}: {line}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// #346 B346-R1-08: string warnings, member outcomes and plan first turns
    /// are shown without --json.
    #[test]
    fn warnings_outcomes_and_first_turns_reach_the_text() {
        let down = render(&json!({
            "pod": "build",
            "members": [
                {"slot": "lead", "seat": "pij-a", "outcome": "not closed: E-RS-NOT-STOPPED"},
                {"slot": "reviewer", "seat": "pij-b", "outcome": "left running (keep-open)"},
            ],
            "warnings": ["pij-b is keep-open, so it was left running"],
        }));
        assert!(down.contains("  ! pij-b is keep-open"), "{down}");
        assert!(!down.contains("! -"), "{down}");
        assert!(
            down.contains("lead: pij-a: not closed: E-RS-NOT-STOPPED"),
            "{down}"
        );
        assert!(!down.contains("started"), "{down}");
        let plan = render(&json!({
            "pod": "build",
            "template": {"source": "pods/build", "version": "1"},
            "project": "fleet",
            "slots": [{"slot": "lead", "role": "notes", "harness": "omp", "first_turn": "You hold notes.\nOne note per call."}],
            "warnings": [{"line": "slot reviewer: shares a model. Fix: edit"}],
        }));
        assert!(plan.contains("      One note per call."), "{plan}");
        assert!(
            plan.contains("  ! slot reviewer: shares a model. Fix: edit"),
            "{plan}"
        );
    }
}
