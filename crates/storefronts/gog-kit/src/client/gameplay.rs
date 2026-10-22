//! What the account did in its games, from `gameplay.gog.com`, as Heroic's
//! `games.ts` reads it with the account's own token: each game's playtime and
//! achievements.
use std::{collections::BTreeMap, time::Duration};

use futures_util::{StreamExt, TryStreamExt, stream};
use obelisk_core::date::unix_seconds;
use reqwest::StatusCode;
use serde::Deserialize;

use super::{Client, valid_product_id};
use crate::{
    GogError,
    auth::Api,
    http::{bounded_json, check_status, safe_https_url, send_retrying},
    library::endpoint,
};

/// Concurrent playtime requests: GOG answers one game at a time.
const PLAYTIMES: usize = 12;

/// One achievement of a game, as the account stands with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Achievement {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Its unlocked icon once unlocked, else its locked one.
    pub icon_url: String,
    pub hidden: bool,
    pub unlocked: bool,
    /// Seconds since the Unix epoch.
    pub unlock_time: Option<u64>,
}

#[derive(Deserialize)]
struct Achievements {
    #[serde(default)]
    items: Vec<RawAchievement>,
}

#[derive(Deserialize)]
struct RawAchievement {
    #[serde(default)]
    achievement_id: String,
    #[serde(default)]
    achievement_key: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    image_url_locked: String,
    #[serde(default)]
    image_url_unlocked: String,
    #[serde(default = "visible")]
    visible: bool,
    #[serde(default)]
    date_unlocked: Option<String>,
}

fn visible() -> bool {
    true
}

impl From<RawAchievement> for Achievement {
    fn from(raw: RawAchievement) -> Self {
        let unlock_time = raw.date_unlocked.as_deref().and_then(unix_seconds);
        let unlocked = raw.date_unlocked.is_some();
        let icon = match unlocked {
            true => &raw.image_url_unlocked,
            false => &raw.image_url_locked,
        };
        Self {
            id: match raw.achievement_key.is_empty() {
                true => raw.achievement_id,
                false => raw.achievement_key,
            },
            name: raw.name,
            description: raw.description,
            icon_url: safe_https_url(icon).unwrap_or_default(),
            hidden: !raw.visible,
            unlocked,
            unlock_time,
        }
    }
}

impl Client {
    /// The game's achievements, named in `locale`, such as `fr-FR`. A game
    /// without any has none.
    pub async fn achievements(
        &self,
        product_id: &str,
        locale: &str,
    ) -> Result<Vec<Achievement>, GogError> {
        if !valid_product_id(product_id) || locale.len() > 32 {
            return Err(GogError::Protocol);
        }
        let user_id = self.credentials().await?.user_id;
        let url = endpoint(
            &self.tokens.api.gameplay,
            &["clients", product_id, "users", &user_id, "achievements"],
        )?;
        self.with_access_token(|token| {
            let url = url.clone();
            async move {
                let response = self
                    .tokens
                    .api
                    .http
                    .get(url)
                    .bearer_auth(&token)
                    .header("X-Gog-Lc", locale)
                    .send()
                    .await
                    .map_err(|_| GogError::Network)?;
                if response.status() == StatusCode::NOT_FOUND {
                    return Ok(Vec::new());
                }
                check_status(response.status())?;
                let achievements: Achievements = bounded_json(response, 8 * 1024 * 1024).await?;
                Ok(achievements.items.into_iter().map(Into::into).collect())
            }
        })
        .await
    }

    /// How many minutes the account played each of `product_ids`, by product.
    /// A game never played has no entry.
    pub async fn playtimes(
        &self,
        product_ids: &[String],
    ) -> Result<BTreeMap<String, u64>, GogError> {
        let user_id = self.credentials().await?.user_id;
        let user_id = user_id.as_str();
        self.with_access_token(|token| async move {
            let requests: Vec<_> = product_ids
                .iter()
                .filter(|id| valid_product_id(id))
                .map(|id| playtime(&self.tokens.api, &token, user_id, id))
                .collect();
            let played: Vec<Option<(String, u64)>> = stream::iter(requests)
                .buffer_unordered(PLAYTIMES)
                .try_collect()
                .await?;
            Ok(played.into_iter().flatten().collect())
        })
        .await
    }
}

/// The minutes the account played `product_id`, if any. A failed request is
/// tried again once.
async fn playtime(
    api: &Api,
    access_token: &str,
    user_id: &str,
    product_id: &str,
) -> Result<Option<(String, u64)>, GogError> {
    #[derive(Deserialize)]
    struct Sessions {
        #[serde(default)]
        time_sum: u64,
    }
    let url = endpoint(
        &api.gameplay,
        &["games", product_id, "users", user_id, "sessions"],
    )?;
    let request = || api.http.get(url.clone()).bearer_auth(access_token);
    let Some(response) = send_retrying(request, 2, Duration::from_millis(500)).await? else {
        return Ok(None);
    };
    let sessions: Sessions = bounded_json(response, 64 * 1024).await?;
    Ok((sessions.time_sum > 0).then(|| (product_id.to_owned(), sessions.time_sum)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn achievements_read_as_the_account_stands_with_them() {
        let achievements: Achievements = serde_json::from_value(json!({"total_count": 2, "items": [
            {"achievement_id": "1", "achievement_key": "kill_boss", "name": "Boss", "description": "Beat it",
             "image_url_locked": "https://images.gog.com/locked.png",
             "image_url_unlocked": "https://images.gog.com/unlocked.png", "visible": true,
             "date_unlocked": "2023-04-01T12:00:00+0000", "rarity": 12.5},
            {"achievement_id": "2", "achievement_key": "", "name": "Secret", "description": "",
             "image_url_locked": "http://images.gog.com/locked.png", "image_url_unlocked": "",
             "visible": false, "date_unlocked": null},
        ]}))
        .unwrap();
        let achievements: Vec<Achievement> =
            achievements.items.into_iter().map(Into::into).collect();
        assert_eq!(
            achievements[0],
            Achievement {
                id: "kill_boss".into(),
                name: "Boss".into(),
                description: "Beat it".into(),
                icon_url: "https://images.gog.com/unlocked.png".into(),
                hidden: false,
                unlocked: true,
                unlock_time: Some(1_680_350_400),
            }
        );
        assert_eq!(achievements[1].id, "2");
        assert!(achievements[1].hidden && !achievements[1].unlocked);
        assert_eq!(achievements[1].icon_url, "", "an insecure icon is dropped");
    }
}
