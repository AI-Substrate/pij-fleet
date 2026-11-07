//! Mid-turn steering, per seat (plan 214, ruling C31; migration 0043).
//!
//! On by default: a seat with no row steers. `pij steer off` records who
//! turned it off and when; `pij steer on` records the same with `steer = 1`.

use pij_core::error::Result;
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};

/// A seat whose steering was set explicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteeringSetting {
    /// The seat.
    pub seat: String,
    /// Whether it steers normal sends into a running turn.
    pub steer: bool,
    /// Who set it.
    pub set_by: String,
    /// When, ms since the epoch.
    pub set_at_ms: u64,
}

impl SqliteOrchestration {
    /// Does `seat` steer normal sends into a running turn? On unless set off.
    /// One primary-key read.
    pub async fn mid_turn_steering(&self, seat: &str) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let steer: Option<i64> =
            sqlx::query_scalar("SELECT steer FROM seat_steering WHERE seat = ?1")
                .bind(seat)
                .fetch_optional(&self.pool)
                .await
                .map_err(adapter_error)?;
        Ok(steer.is_none_or(|steer| steer != 0))
    }

    /// Set `seat`'s steering; `true` when that changed it.
    pub async fn set_mid_turn_steering(
        &self,
        seat: &str,
        steer: bool,
        set_by: &str,
        at_ms: u64,
    ) -> Result<bool> {
        let was = self.mid_turn_steering(seat).await?;
        let pool = self.pool.clone();
        let (seat, set_by) = (seat.to_string(), set_by.to_string());
        let at = sql_i64(at_ms, "set_at_ms")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            sqlx::query(
                "INSERT INTO seat_steering (seat, steer, set_by, set_at_ms) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT (seat) DO UPDATE SET steer = excluded.steer, \
                 set_by = excluded.set_by, set_at_ms = excluded.set_at_ms",
            )
            .bind(&seat)
            .bind(i64::from(steer))
            .bind(&set_by)
            .bind(at)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(())
        })
        .await?;
        Ok(was != steer)
    }

    /// Every explicit setting, by seat.
    pub async fn steering_settings(&self) -> Result<Vec<SteeringSetting>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT seat, steer, set_by, set_at_ms FROM seat_steering ORDER BY seat")
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .iter()
            .map(|row| {
                let steer: i64 = row.try_get("steer").map_err(adapter_error)?;
                let at: i64 = row.try_get("set_at_ms").map_err(adapter_error)?;
                Ok(SteeringSetting {
                    seat: row.try_get("seat").map_err(adapter_error)?,
                    steer: steer != 0,
                    set_by: row.try_get("set_by").map_err(adapter_error)?,
                    set_at_ms: sql_u64(at, "set_at_ms")?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::SqliteOrchestration;

    #[tokio::test]
    async fn steering_is_on_until_set_off_and_reports_a_change_once() {
        let store = SqliteOrchestration::new(crate::open("").await.expect("memory"));
        assert!(
            store.mid_turn_steering("pij-a").await.unwrap(),
            "on by default"
        );
        assert!(
            store
                .set_mid_turn_steering("pij-a", false, "pij-pm", 10)
                .await
                .unwrap()
        );
        assert!(!store.mid_turn_steering("pij-a").await.unwrap());
        assert!(
            !store
                .set_mid_turn_steering("pij-a", false, "pij-pm", 11)
                .await
                .unwrap()
        );
        assert!(store.mid_turn_steering("pij-b").await.unwrap(), "per seat");
        assert!(
            store
                .set_mid_turn_steering("pij-a", true, "pij-pa", 12)
                .await
                .unwrap()
        );
        let settings = store.steering_settings().await.unwrap();
        assert_eq!(
            (
                settings.len(),
                settings[0].steer,
                settings[0].set_by.as_str()
            ),
            (1, true, "pij-pa")
        );
    }
}
