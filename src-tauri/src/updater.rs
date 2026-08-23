use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::{Updater, UpdaterExt};

const UPDATE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
struct ReleaseUpdateConfiguration {
    endpoint: tauri::Url,
    public_key: String,
}

impl ReleaseUpdateConfiguration {
    fn embedded() -> Result<Self, String> {
        Self::from_values(
            option_env!("SLICKCLIP_UPDATER_ENDPOINT"),
            option_env!("SLICKCLIP_UPDATER_PUBLIC_KEY"),
        )
    }

    fn from_values(endpoint: Option<&str>, public_key: Option<&str>) -> Result<Self, String> {
        let endpoint = endpoint
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "This build does not include a release update endpoint.".to_string())?;
        let public_key = public_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "This build does not include an updater verification key.".to_string()
            })?;
        let endpoint = endpoint
            .parse::<tauri::Url>()
            .map_err(|error| format!("The embedded update endpoint is invalid: {error}"))?;
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(
                "The embedded update endpoint must be an HTTPS URL without credentials or a fragment."
                    .to_string(),
            );
        }
        Ok(Self {
            endpoint,
            public_key: public_key.to_string(),
        })
    }
}

#[derive(Clone, Default)]
pub struct UpdateManager {
    active: Arc<AtomicBool>,
}

impl UpdateManager {
    fn begin(&self) -> Result<UpdateOperation, String> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "Another update operation is already running.".to_string())?;
        Ok(UpdateOperation {
            active: Arc::clone(&self.active),
        })
    }
}

struct UpdateOperation {
    active: Arc<AtomicBool>,
}

impl Drop for UpdateOperation {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConfigurationResponse {
    configured: bool,
    current_version: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResponse {
    current_version: String,
    update_available: bool,
    version: Option<String>,
    notes: Option<String>,
    published_at: Option<String>,
}

#[tauri::command]
pub fn get_update_configuration(app: AppHandle) -> UpdateConfigurationResponse {
    match ReleaseUpdateConfiguration::embedded() {
        Ok(_) => UpdateConfigurationResponse {
            configured: true,
            current_version: app.package_info().version.to_string(),
            message: "Signed release updates are configured for this build.".to_string(),
        },
        Err(message) => UpdateConfigurationResponse {
            configured: false,
            current_version: app.package_info().version.to_string(),
            message,
        },
    }
}

#[tauri::command]
pub async fn check_for_slickclip_update(
    app: AppHandle,
    manager: tauri::State<'_, UpdateManager>,
) -> Result<UpdateCheckResponse, String> {
    let _operation = manager.begin()?;
    let updater = configured_updater(&app)?;
    let update = updater
        .check()
        .await
        .map_err(|error| format!("SlickClip could not check for updates: {error}"))?;
    Ok(match update {
        Some(update) => UpdateCheckResponse {
            current_version: update.current_version,
            update_available: true,
            version: Some(update.version),
            notes: update.body,
            published_at: update.date.map(|date| date.to_string()),
        },
        None => UpdateCheckResponse {
            current_version: app.package_info().version.to_string(),
            update_available: false,
            version: None,
            notes: None,
            published_at: None,
        },
    })
}

#[tauri::command]
pub async fn install_slickclip_update(
    app: AppHandle,
    manager: tauri::State<'_, UpdateManager>,
    expected_version: String,
) -> Result<(), String> {
    let expected_version = validate_expected_version(&expected_version)?;
    let _operation = manager.begin()?;
    let updater = configured_updater(&app)?;
    let update = updater
        .check()
        .await
        .map_err(|error| format!("SlickClip could not re-check the update: {error}"))?
        .ok_or_else(|| "The selected update is no longer available.".to_string())?;
    if update.version != expected_version {
        return Err(format!(
            "The available release changed from {expected_version} to {}. Check again before installing.",
            update.version
        ));
    }
    let verified_installer = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|error| format!("SlickClip could not download or verify the update: {error}"))?;
    crate::prepare_for_exit(&app);
    update
        .install(verified_installer)
        .map_err(|error| format!("SlickClip could not install the verified update: {error}"))
}

fn configured_updater(app: &AppHandle) -> Result<Updater, String> {
    let configuration = ReleaseUpdateConfiguration::embedded()?;
    app.updater_builder()
        .pubkey(configuration.public_key)
        .endpoints(vec![configuration.endpoint])
        .map_err(|error| format!("The embedded update endpoint was rejected: {error}"))?
        .timeout(UPDATE_TIMEOUT)
        .build()
        .map_err(|error| format!("SlickClip could not initialize signed updates: {error}"))
}

fn validate_expected_version(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        return Err("The selected update version is invalid. Check for updates again.".to_string());
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_configuration_requires_both_trust_inputs_and_https() {
        assert!(ReleaseUpdateConfiguration::from_values(None, None).is_err());
        assert!(ReleaseUpdateConfiguration::from_values(
            Some("https://releases.example.test/latest.json"),
            None,
        )
        .is_err());
        assert!(ReleaseUpdateConfiguration::from_values(
            Some("http://releases.example.test/latest.json"),
            Some("public-key"),
        )
        .is_err());
        assert!(ReleaseUpdateConfiguration::from_values(
            Some("https://user:password@releases.example.test/latest.json"),
            Some("public-key"),
        )
        .is_err());

        let configured = ReleaseUpdateConfiguration::from_values(
            Some("https://releases.example.test/{{target}}/{{arch}}/{{current_version}}"),
            Some("public-key"),
        )
        .unwrap();
        assert_eq!(configured.endpoint.scheme(), "https");
        assert_eq!(configured.public_key, "public-key");
    }

    #[test]
    fn expected_version_is_bounded_and_contains_only_semver_characters() {
        assert_eq!(validate_expected_version(" 1.0.1 ").unwrap(), "1.0.1");
        assert_eq!(
            validate_expected_version("1.1.0-rc.1+windows").unwrap(),
            "1.1.0-rc.1+windows"
        );
        assert!(validate_expected_version("").is_err());
        assert!(validate_expected_version("1.0.1 /S").is_err());
        assert!(validate_expected_version(&"1".repeat(65)).is_err());
    }

    #[test]
    fn update_manager_allows_only_one_operation() {
        let manager = UpdateManager::default();
        let operation = manager.begin().unwrap();
        assert!(manager.begin().is_err());
        drop(operation);
        assert!(manager.begin().is_ok());
    }
}
