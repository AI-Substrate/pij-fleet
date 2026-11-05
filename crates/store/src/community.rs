//! The community directory pointer (plan 190, migration 0031).
//!
//! The governance branch is the truth. These rows say where each document pij
//! wrote lives and which commit last wrote it, and can be rebuilt from the
//! branch head at any time.

use pij_core::error::Result;
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error};

/// One document in a project's community directory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommunityPointer {
    /// The project slug.
    pub project: String,
    /// Path relative to the governance worktree's top level.
    pub path: String,
    /// The commit that last wrote it.
    pub commit: String,
}

impl SqliteOrchestration {
    /// Record or move the pointers for documents a write just committed.
    pub async fn record_community_pointers(&self, pointers: &[CommunityPointer]) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let pointers = pointers.to_vec();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            for pointer in &pointers {
                upsert(&mut tx, pointer).await?;
            }
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }

    /// Replace every pointer for `project` with `pointers`, in one transaction:
    /// the rebuild from the governance branch head.
    pub async fn replace_community_pointers(
        &self,
        project: &str,
        pointers: &[CommunityPointer],
    ) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let project = project.to_string();
        let pointers = pointers.to_vec();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            sqlx::query("DELETE FROM community_pointers WHERE project = ?1")
                .bind(&project)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
            for pointer in pointers.iter().filter(|pointer| pointer.project == project) {
                upsert(&mut tx, pointer).await?;
            }
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }

    /// Every pointer for `project`, by path.
    pub async fn community_pointers(&self, project: &str) -> Result<Vec<CommunityPointer>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(
            "SELECT project, path, commit_sha FROM community_pointers \
             WHERE project = ?1 ORDER BY path",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(CommunityPointer {
                    project: row.try_get("project").map_err(adapter_error)?,
                    path: row.try_get("path").map_err(adapter_error)?,
                    commit: row.try_get("commit_sha").map_err(adapter_error)?,
                })
            })
            .collect()
    }
}

async fn upsert(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    pointer: &CommunityPointer,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO community_pointers (project, path, commit_sha) VALUES (?1, ?2, ?3) \
         ON CONFLICT (project, path) DO UPDATE SET commit_sha = excluded.commit_sha",
    )
    .bind(&pointer.project)
    .bind(&pointer.path)
    .bind(&pointer.commit)
    .execute(&mut **tx)
    .await
    .map_err(adapter_error)?;
    Ok(())
}
