//! GOG's account seam. Only refresh credentials cross into the macOS Keychain;
//! the account access token stays inside the Rust session.
use std::{collections::HashMap, sync::Arc};

use crate::{
    GamePlatform, LibraryCache,
    library_cache::{self, GameAchievements, GogLibrary, LibraryAchievement},
};
use gog_kit::{auth, client};

/// Achievement requests a library sends at once: GOG answers one game at a
/// time.
const ACHIEVEMENT_READS: usize = 6;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum GogError {
    #[error("backend.gog.network")]
    Network,
    #[error("backend.gog.unauthorized")]
    Unauthorized,
    #[error("backend.gog.invalid_code")]
    InvalidCode,
    #[error("backend.gog.invalid_response")]
    InvalidResponse,
    #[error("{message}")]
    Service { message: String },
}

impl From<gog_kit::GogError> for GogError {
    fn from(error: gog_kit::GogError) -> Self {
        match error {
            gog_kit::GogError::Network => Self::Network,
            gog_kit::GogError::Unauthorized => Self::Unauthorized,
            gog_kit::GogError::InvalidCode => Self::InvalidCode,
            gog_kit::GogError::Protocol => Self::InvalidResponse,
            gog_kit::GogError::Service { message } => Self::Service { message },
            error => Self::Service {
                message: error.to_string(),
            },
        }
    }
}

#[uniffi::export]
pub fn gog_login_url() -> String {
    auth::login_url()
}

/// Whether the sign-in window reached the page that carries GOG's code.
#[uniffi::export]
pub fn gog_is_login_redirect(url: String) -> bool {
    auth::is_redirect(&url)
}

#[uniffi::export]
pub fn gog_authorization_code(input: String) -> Result<String, GogError> {
    auth::authorization_code(&input).map_err(Into::into)
}

#[derive(Clone, uniffi::Record)]
pub struct GogCredentials {
    pub user_id: String,
    pub username: String,
    pub refresh_token: String,
}

impl From<auth::Credentials> for GogCredentials {
    fn from(credentials: auth::Credentials) -> Self {
        Self {
            user_id: credentials.user_id,
            username: credentials.username,
            refresh_token: credentials.refresh_token,
        }
    }
}

impl From<GogCredentials> for auth::Credentials {
    fn from(credentials: GogCredentials) -> Self {
        Self {
            user_id: credentials.user_id,
            username: credentials.username,
            refresh_token: credentials.refresh_token,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct GogGame {
    pub product_id: String,
    pub title: String,
    /// The portrait cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// The wide artwork the hero and the game's page show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_image_url: Option<String>,
    pub windows: bool,
    pub macos: bool,
    /// In English: gamesdb has no other language.
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub developers: Vec<String>,
    #[serde(default)]
    pub publishers: Vec<String>,
    /// Seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<u64>,
    #[serde(default)]
    pub genres: Vec<String>,
    /// The critics' rating, out of 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<u32>,
    /// The languages the game speaks, such as `fr-FR`.
    #[serde(default)]
    pub languages: Vec<String>,
    /// When the game joined the account's library, in seconds since the
    /// Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[uniffi(default = None)]
    pub added_at: Option<u64>,
}

impl From<client::Game> for GogGame {
    fn from(game: client::Game) -> Self {
        let info = game.info;
        Self {
            product_id: game.product_id,
            title: game.title,
            image_url: game.image_url,
            header_image_url: game.header_image_url,
            windows: game.windows,
            macos: game.macos,
            summary: info.summary,
            developers: info.developers,
            publishers: info.publishers,
            release_date: info.release_date,
            genres: info.genres,
            rating: info.rating,
            languages: info.languages,
            added_at: game.added_at,
        }
    }
}

/// How long the account played one game, as GOG records it.
#[derive(Clone, Debug, uniffi::Record)]
pub struct GogPlaytime {
    pub product_id: String,
    pub minutes: u64,
}

impl From<client::Achievement> for LibraryAchievement {
    fn from(value: client::Achievement) -> Self {
        Self {
            id: value.id,
            name: value.name,
            description: value.description,
            icon_url: value.icon_url,
            hidden: value.hidden,
            unlocked: value.unlocked,
            unlocked_at: value.unlock_time,
        }
    }
}

/// What a game's page adds to its library entry: the build it shows before
/// any download, and its size.
#[derive(Clone, Debug, uniffi::Record)]
pub struct GogGameDetails {
    /// Unknown when GOG could not tell.
    pub platform: Option<GamePlatform>,
    /// Unknown when GOG serves no build for the platform.
    pub installation_bytes: Option<u64>,
}

impl From<client::GameDetails> for GogGameDetails {
    fn from(details: client::GameDetails) -> Self {
        Self {
            platform: details.platform.map(Into::into),
            installation_bytes: details.installation_bytes,
        }
    }
}

#[derive(uniffi::Object)]
pub struct GogSession {
    pub(crate) client: Arc<client::Client>,
    pub(crate) persistence: Arc<crate::credentials::Persistence>,
    /// Where the account's library is kept as it loads.
    library: Option<GogLibrary>,
}

impl GogSession {
    async fn with(
        client: client::Client,
        library: Option<Arc<LibraryCache>>,
    ) -> Result<Arc<Self>, GogError> {
        let library = match library {
            Some(library) => Some(library.gog(&client.credentials().await?.user_id)),
            None => None,
        };
        Ok(Arc::new(Self {
            client: Arc::new(client),
            persistence: Default::default(),
            library,
        }))
    }

    /// `product_ids`' achievements in `locale`: the library's for a game not
    /// played since they were read, unless `fresh`, else GOG's, which answers
    /// one game at a time, which the library then keeps. A game GOG did not
    /// answer for is left out; when none could be told, the first failure.
    async fn read_achievements(
        &self,
        product_ids: Vec<String>,
        locale: &str,
        fresh: bool,
    ) -> Result<Vec<GameAchievements>, GogError> {
        let client = &self.client;
        library_cache::achievements(
            self.library.as_ref().map(|library| library.achievements()),
            product_ids,
            locale,
            fresh,
            ACHIEVEMENT_READS,
            |product_id| async move {
                client
                    .achievements(&product_id, locale)
                    .await
                    .map_err(GogError::from)
            },
        )
        .await
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl GogSession {
    /// Keeps every library it loads in `library`.
    #[uniffi::constructor]
    pub async fn sign_in(
        authorization_code: String,
        library: Option<Arc<LibraryCache>>,
    ) -> Result<Arc<Self>, GogError> {
        Self::with(client::Client::sign_in(&authorization_code).await?, library).await
    }

    /// Keeps every library it loads in `library`.
    #[uniffi::constructor]
    pub async fn restore(
        credentials: GogCredentials,
        library: Option<Arc<LibraryCache>>,
    ) -> Result<Arc<Self>, GogError> {
        Self::with(client::Client::restore(credentials.into()).await?, library).await
    }

    pub async fn credentials(&self) -> Result<GogCredentials, GogError> {
        Ok(self.client.credentials().await?.into())
    }

    /// The account's avatar, as its GOG profile shows it.
    pub fn avatar_url(&self) -> Option<String> {
        self.client.avatar_url()
    }

    pub fn shutdown_observers(&self) {
        self.client.shutdown_observers();
    }

    pub async fn next_credentials_revision(&self, after_revision: u64) -> Option<u64> {
        self.client.next_credentials_revision(after_revision).await
    }

    /// Saves the account's credentials through `credentials` now and after
    /// every rotation, one save at a time and in order, until the session's
    /// observers stop. Every account call relies on it.
    pub async fn persist(&self, credentials: Arc<dyn crate::CredentialSave>) {
        crate::credentials::persist(self.client.clone(), self.persistence.clone(), credentials);
    }

    pub async fn games(&self) -> Result<Vec<GogGame>, GogError> {
        let games = self.client.games().await?;
        Ok(match &self.library {
            Some(library) => library.games(games),
            None => games.into_iter().map(Into::into).collect(),
        })
    }

    /// The game's achievements in `locale`, such as `fr-FR`, as the game
    /// page reads them: the library's while the game was not played since
    /// they were read, unless `fresh`, else GOG's, which the library then
    /// keeps.
    pub async fn achievements(
        &self,
        product_id: String,
        locale: String,
        fresh: bool,
    ) -> Result<Vec<LibraryAchievement>, GogError> {
        let read = self
            .read_achievements(vec![product_id], &locale, fresh)
            .await?;
        Ok(read
            .into_iter()
            .next()
            .map(|game| game.achievements)
            .unwrap_or_default())
    }

    /// The share of each of `product_ids`' achievements the account unlocked,
    /// by product, as `achievements` reads them: none for a game without
    /// achievements or that GOG did not answer for. It fails only when no
    /// game could be told.
    pub async fn achievement_shares(
        &self,
        product_ids: Vec<String>,
        locale: String,
    ) -> Result<HashMap<String, f64>, GogError> {
        let read = self.read_achievements(product_ids, &locale, false).await?;
        Ok(library_cache::unlocked_shares(read).collect())
    }

    /// How long the account played each of `product_ids`; a game never
    /// played has no entry. GOG answers one game at a time.
    pub async fn playtimes(&self, product_ids: Vec<String>) -> Result<Vec<GogPlaytime>, GogError> {
        let playtimes = self.client.playtimes(&product_ids).await?;
        if let Some(library) = &self.library {
            library.playtimes(&playtimes);
        }
        Ok(playtimes
            .into_iter()
            .map(|(product_id, minutes)| GogPlaytime {
                product_id,
                minutes,
            })
            .collect())
    }

    pub async fn disconnect(&self) -> Result<(), GogError> {
        self.client.disconnect().await.map_err(Into::into)
    }

    /// `language` names the depots, such as `fr-FR`.
    pub async fn game_details(
        &self,
        product_id: String,
        language: String,
    ) -> Result<GogGameDetails, GogError> {
        Ok(self
            .client
            .game_details(&product_id, &language)
            .await?
            .into())
    }
}
