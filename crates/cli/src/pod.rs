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
        lines.extend(
            warnings
                .iter()
                .map(|warning| format!("  ! {}", text(&warning["line"]))),
        );
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
        }
    }
    if let Some(members) = data["members"].as_array() {
        lines.push("members:".to_string());
        for member in members {
            lines.push(format!(
                "  {}: {} in {} ({}), canary {}",
                text(&member["slot"]),
                text(&member["seat"]),
                text(&member["role"]),
                member["state"].as_str().unwrap_or("started"),
                text(&member["canary"])
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
