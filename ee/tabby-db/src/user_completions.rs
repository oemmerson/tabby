use std::time::Duration;

use anyhow::{Context, Result};
use cached::CachedAsync;
use chrono::{DateTime, Utc};
use sqlx::{prelude::FromRow, query};

use crate::{DateTimeUtc, DbConn};

#[derive(FromRow)]
pub struct UserCompletionDAO {
    pub user_id: i64,
    pub completion_id: String,
    pub language: String,

    pub views: i64,
    pub selects: i64,
    pub dismisses: i64,

    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(FromRow, Clone)]
pub struct UserCompletionDailyStatsDAO {
    pub start: DateTime<Utc>,
    pub completions: i32,
    pub selects: i32,
}

impl DbConn {
    pub async fn create_user_completion(
        &self,
        ts: u128,
        user_id: i64,
        completion_id: String,
        language: String,
    ) -> Result<i32> {
        let duration = Duration::from_millis(ts as u64);
        let created_at =
            DateTime::from_timestamp(duration.as_secs() as i64, duration.subsec_nanos())
                .context("Invalid created_at timestamp")?;
        let res = query!(
            "INSERT INTO user_completions (user_id, completion_id, language, created_at) VALUES (?, ?, ?, ?);",
            user_id,
            completion_id,
            language,
            created_at
        )
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid() as i32)
    }

    pub async fn add_to_user_completion(
        &self,
        ts: u128,
        completion_id: &str,
        views: i64,
        selects: i64,
        dismisses: i64,
    ) -> Result<()> {
        let duration = Duration::from_millis(ts as u64);
        let updated_at =
            DateTime::from_timestamp(duration.as_secs() as i64, duration.subsec_nanos())
                .context("Invalid updated_at timestamp")?;
        query!("UPDATE user_completions SET views = views + ?, selects = selects + ?, dismisses = dismisses + ?, updated_at = ? WHERE completion_id = ?",
            views, selects, dismisses, updated_at, completion_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn compute_daily_stats_in_past_year(
        &self,
        users: Vec<i64>,
    ) -> Result<Vec<UserCompletionDailyStatsDAO>> {
        if users.len() <= 1 {
            let mut cache = self.cache.daily_stats_in_past_year.lock().await;
            let key = if users.is_empty() {
                None
            } else {
                Some(users[0])
            };
            return Ok(cache
                .try_get_or_set_with(key, move || async move {
                    self.compute_daily_stats_in_past_year_impl(users).await
                })
                .await?
                .clone());
        }
        self.compute_daily_stats_in_past_year_impl(users).await
    }

    async fn compute_daily_stats_in_past_year_impl(
        &self,
        users: Vec<i64>,
    ) -> Result<Vec<UserCompletionDailyStatsDAO>> {
        let users = users
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        Ok(sqlx::query_as(&format!(
            r#"
            SELECT CAST(STRFTIME('%s', DATE(created_at)) AS TIMESTAMP) as start,
                   SUM(1) as completions,
                   SUM(selects) as selects
            FROM user_completions
            WHERE created_at >= DATE('now', '-1 year')
                AND ({users_empty} OR user_id IN ({users}))
            GROUP BY 1
            ORDER BY 1 ASC
            "#,
            users_empty = users.is_empty(),
        ))
        .bind(users)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn compute_daily_stats(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        users: Vec<i64>,
        languages: Vec<String>,
        all_languages: Vec<String>,
    ) -> Result<Vec<UserCompletionDailyStatsDAO>> {
        let users = users
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let languages = languages
            .into_iter()
            .map(|l| format!("'{}'", l))
            .collect::<Vec<_>>()
            .join(",");
        let all_languages = all_languages
            .into_iter()
            .map(|l| format!("'{}'", l))
            .collect::<Vec<_>>()
            .join(",");

        // Groups stats by day, using `DATE(created_at)` to extract the day and converting it into seconds
        // with `STRFTIME('%s')`. The effect of this is to extract the unix timestamp (seconds) rounded to
        // the start of the day and group them by that.
        let res = sqlx::query_as(&format!(
            r#"
            SELECT CAST(STRFTIME('%s', DATE(created_at)) AS TIMESTAMP) as start,
                   SUM(1) as completions,
                   SUM(selects) as selects
            FROM (
                SELECT created_at, selects, IIF(language IN ({all_languages}), language, 'other') as language
                    FROM user_completions
                    WHERE created_at >= ?1 AND created_at < ?2
            )
                WHERE ({no_selected_users} OR user_id IN ({users}))
                AND ({no_selected_languages} OR language IN ({languages}))
            GROUP BY 1
            ORDER BY 1 ASC
            "#,
            no_selected_users = users.is_empty(),
            no_selected_languages = languages.is_empty(),
        ))
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;
        Ok(res)
    }

    #[cfg(any(test, feature = "testutils"))]
    pub async fn fetch_one_user_completion(&self) -> Result<Option<UserCompletionDAO>> {
        Ok(
            sqlx::query_as!(UserCompletionDAO, "SELECT user_id, completion_id, language, created_at, updated_at, views, selects, dismisses FROM user_completions")
                .fetch_optional(&self.pool)
                .await?,
        )
    }
}
