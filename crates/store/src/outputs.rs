//! Community outputs (plan 200, migration 0036).
//!
//! The store's index of each `pij output` the writer committed: a verdict,
//! deliverable, note or handover under `roles/<id>/outputs/<yyyy-mm>/`. The
//! governance branch holds the documents and stays the truth; this table answers
//! `pij outputs` for an exact head or a role without reading the branch.

use pij_core::error::{PijError, Result};
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};

/// The output kinds the pij/output schema allows.
pub const OUTPUT_KINDS: [&str; 4] = ["verdict", "deliverable", "note", "handover"];

/// One committed output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunityOutput {
    /// The document, relative to the governance worktree.
    pub path: String,
    /// The project slug.
    pub project: String,
    /// The role it was written for.
    pub role: String,
    /// `verdict`, `deliverable`, `note` or `handover`.
    pub kind: String,
    /// What it is about, such as `pr:132`.
    pub subject: String,
    /// The exact head a verdict is for.
    pub head: Option<String>,
    /// A verdict's result: `pass`, `pass-with-notes` or `block`.
    pub result: Option<String>,
    /// The writing seat.
    pub writer: String,
    /// Whether the writer held the role when it wrote.
    pub holds_role: bool,
    /// The request it answers (ruling C3).
    pub answers: Option<String>,
    /// The governance commit that holds it.
    pub commit: String,
    /// When it was written, ms since the epoch.
    pub created_at: u64,
}

const COLUMNS: &str = "path, project, role, kind, subject, head, result, writer, holds_role, \
     answers, commit_sha, created_at";

impl SqliteOrchestration {
    /// Index one committed output. One already indexed for its project and
    /// path is left as it is; another project may hold the same relative path.
    ///
    /// # Errors
    /// Store failures, or an out-of-range timestamp.
    pub async fn record_output(&self, output: &CommunityOutput) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let output = output.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            sqlx::query(&format!(
                "INSERT INTO community_outputs ({COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                 ON CONFLICT (project, path) DO NOTHING"
            ))
            .bind(&output.path)
            .bind(&output.project)
            .bind(&output.role)
            .bind(&output.kind)
            .bind(&output.subject)
            .bind(&output.head)
            .bind(&output.result)
            .bind(&output.writer)
            .bind(i64::from(output.holds_role))
            .bind(&output.answers)
            .bind(&output.commit)
            .bind(sql_i64(output.created_at, "created_at")?)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }

    /// `project`'s outputs about `subject`, newest first; with `head`, only
    /// those for exactly that head, so a moved head answers none.
    ///
    /// # Errors
    /// Store failures.
    pub async fn outputs_for_subject(
        &self,
        project: &str,
        subject: &str,
        head: Option<&str>,
    ) -> Result<Vec<CommunityOutput>> {
        require_current_schema(&self.pool).await?;
        let rows = match head {
            Some(head) => {
                sqlx::query(&format!(
                    "SELECT {COLUMNS} FROM community_outputs \
                     WHERE project = ?1 AND subject = ?2 AND head = ?3 \
                     ORDER BY created_at DESC, path DESC"
                ))
                .bind(project)
                .bind(subject)
                .bind(head)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query(&format!(
                    "SELECT {COLUMNS} FROM community_outputs \
                     WHERE project = ?1 AND subject = ?2 \
                     ORDER BY created_at DESC, path DESC"
                ))
                .bind(project)
                .bind(subject)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(adapter_error)?;
        rows.iter().map(output).collect()
    }

    /// `project`'s outputs for `role`, newest first, at most `limit`.
    ///
    /// # Errors
    /// Store failures.
    pub async fn outputs_for_role(
        &self,
        project: &str,
        role: &str,
        limit: u32,
    ) -> Result<Vec<CommunityOutput>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(&format!(
            "SELECT {COLUMNS} FROM community_outputs WHERE project = ?1 AND role = ?2 \
             ORDER BY created_at DESC, path DESC LIMIT ?3"
        ))
        .bind(project)
        .bind(role)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?
        .iter()
        .map(output)
        .collect()
    }
}

fn output(row: &sqlx::sqlite::SqliteRow) -> Result<CommunityOutput> {
    let text = |name: &str| -> Result<String> { row.try_get(name).map_err(adapter_error) };
    let optional =
        |name: &str| -> Result<Option<String>> { row.try_get(name).map_err(adapter_error) };
    let holds: i64 = row.try_get("holds_role").map_err(adapter_error)?;
    let created_at: i64 = row.try_get("created_at").map_err(adapter_error)?;
    let kind = text("kind")?;
    if !OUTPUT_KINDS.contains(&kind.as_str()) {
        return Err(PijError::Adapter {
            adapter: "store/outputs".to_string(),
            message: format!("output kind {kind:?} is not a known value"),
        });
    }
    Ok(CommunityOutput {
        path: text("path")?,
        project: text("project")?,
        role: text("role")?,
        kind,
        subject: text("subject")?,
        head: optional("head")?,
        result: optional("result")?,
        writer: text("writer")?,
        holds_role: holds != 0,
        answers: optional("answers")?,
        commit: text("commit_sha")?,
        created_at: sql_u64(created_at, "created_at")?,
    })
}
