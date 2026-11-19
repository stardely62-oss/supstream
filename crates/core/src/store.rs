//! Настройки и токены на диске.
//!
//! * `config.json`  — обычные настройки, их можно смотреть и править руками;
//! * `secrets.json` — токены площадок. TODO: перенести в Windows Credential
//!   Manager (крейт `keyring`), когда дойдём до упаковки.
//!
//! Папка: `%APPDATA%\Supstream\Supstream\data` на Windows, или `SUPSTREAM_DATA_DIR`.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const DEFAULT_PORT: u16 = 7272;

/// Client ID Twitch-приложения можно вшить при сборке:
/// `SUPSTREAM_TWITCH_CLIENT_ID=... cargo build --release`.
/// Тогда друзьям ничего вводить не нужно — у Public-клиента нет секрета.
pub const BUILTIN_TWITCH_CLIENT_ID: Option<&str> = option_env!("SUPSTREAM_TWITCH_CLIENT_ID");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    /// Ключ доступа к локальному API. Генерируется при первом запуске.
    pub api_token: String,
    pub twitch: TwitchConfig,
    pub donationalerts: DaConfig,
    pub music: crate::music::MusicConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            api_token: random_token(),
            twitch: TwitchConfig::default(),
            donationalerts: DaConfig::default(),
            music: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub port: u16,
    /// Слушать локальную сеть (для панели с телефона). По умолчанию — только этот ПК.
    pub allow_lan: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, allow_lan: false }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TwitchConfig {
    /// Пусто — используется вшитый при сборке.
    pub client_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DaConfig {
    pub client_id: String,
    pub client_secret: String,
}

impl Config {
    pub fn twitch_client_id(&self) -> Option<String> {
        let own = self.twitch.client_id.trim();
        if !own.is_empty() {
            return Some(own.to_string());
        }
        BUILTIN_TWITCH_CLIENT_ID.filter(|s| !s.is_empty()).map(str::to_string)
    }

    pub fn da_configured(&self) -> bool {
        !self.donationalerts.client_id.trim().is_empty()
            && !self.donationalerts.client_secret.trim().is_empty()
    }

    pub fn da_redirect_uri(&self) -> String {
        // DA сверяет адрес посимвольно с тем, что указан в настройках приложения.
        format!("http://127.0.0.1:{}/auth/donationalerts/callback", self.server.port)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Secrets {
    pub twitch: Option<TwitchTokens>,
    pub donationalerts: Option<DaTokens>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwitchTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub scopes: Vec<String>,
    pub user_id: String,
    pub login: String,
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub struct Store {
    dir: PathBuf,
    config: RwLock<Config>,
    secrets: RwLock<Secrets>,
}

impl Store {
    pub fn default_dir() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("SUPSTREAM_DATA_DIR") {
            return Ok(PathBuf::from(dir));
        }
        let dirs = directories::ProjectDirs::from("app", "Supstream", "Supstream")
            .context("не удалось определить папку данных")?;
        Ok(dirs.data_dir().to_path_buf())
    }

    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).with_context(|| format!("не удалось создать {}", dir.display()))?;
        let config: Config = load_or_default(&dir.join("config.json"))?;
        let secrets: Secrets = load_or_default(&dir.join("secrets.json"))?;
        let store = Self { dir, config: RwLock::new(config), secrets: RwLock::new(secrets) };
        // Сохраняем сразу: так появится api_token и новые поля по умолчанию.
        store.save_config()?;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn secrets(&self) -> Secrets {
        self.secrets.read().unwrap().clone()
    }

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) -> Result<()> {
        f(&mut self.config.write().unwrap());
        self.save_config()
    }

    pub fn update_secrets(&self, f: impl FnOnce(&mut Secrets)) -> Result<()> {
        f(&mut self.secrets.write().unwrap());
        let snapshot = self.secrets();
        write_atomic(&self.dir.join("secrets.json"), &snapshot)
    }

    fn save_config(&self) -> Result<()> {
        let snapshot = self.config();
        write_atomic(&self.dir.join("config.json"), &snapshot)
    }
}

fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("файл {} повреждён", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("не удалось прочитать {}", path.display())),
    }
}

/// Запись через временный файл: при сбое посреди записи старый файл не портится.
fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("не удалось записать {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("не удалось записать {}", path.display()))?;
    Ok(())
}

pub fn random_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_config_with_token_and_persists() {
        let dir = std::env::temp_dir().join(format!("supstream-test-{}", uuid::Uuid::new_v4()));
        let token = {
            let store = Store::open(&dir).unwrap();
            store.update_config(|c| c.donationalerts.client_id = "123".into()).unwrap();
            store
                .update_secrets(|s| {
                    s.donationalerts = Some(DaTokens {
                        access_token: "a".into(),
                        refresh_token: "r".into(),
                        expires_at: chrono::Utc::now(),
                    })
                })
                .unwrap();
            store.config().api_token
        };
        assert_eq!(token.len(), 64);
        let store = Store::open(&dir).unwrap();
        assert_eq!(store.config().api_token, token);
        assert_eq!(store.config().donationalerts.client_id, "123");
        assert!(store.secrets().donationalerts.is_some());
        std::fs::remove_dir_all(dir).ok();
    }
}
