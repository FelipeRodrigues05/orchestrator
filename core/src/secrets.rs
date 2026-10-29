//! API keys in the Secret Service (ADR 0007). SQLite only ever sees `key_ref`.
//! Keys are never logged; errors carry no secret material.

const APPLICATION: &str = "orchestrator";

/// The reference stored with a provider: `orchestrator/anthropic`.
pub fn key_ref(service: &str) -> String {
    format!("{APPLICATION}/{service}")
}

fn service_of(key_ref: &str) -> &str {
    key_ref.strip_prefix("orchestrator/").unwrap_or(key_ref)
}

fn attributes(service: &str) -> [(&'static str, String); 2] {
    [("application", APPLICATION.to_owned()), ("service", service.to_owned())]
}

pub async fn store(service: &str, key: &str) -> Result<String, String> {
    let keyring = oo7::Keyring::new().await.map_err(|e| e.to_string())?;
    keyring
        .create_item(&format!("Orchestrator API key: {service}"), &attributes(service), key.as_bytes(), true)
        .await
        .map_err(|e| e.to_string())?;
    Ok(key_ref(service))
}

pub async fn load(key_ref: &str) -> Result<Option<String>, String> {
    let keyring = oo7::Keyring::new().await.map_err(|e| e.to_string())?;
    let items = keyring.search_items(&attributes(service_of(key_ref))).await.map_err(|e| e.to_string())?;
    let Some(item) = items.first() else {
        return Ok(None);
    };
    item.unlock().await.map_err(|e| e.to_string())?;
    let secret = item.secret().await.map_err(|e| e.to_string())?;
    Ok(Some(String::from_utf8_lossy(&secret).into_owned()))
}

pub async fn has(service: &str) -> bool {
    matches!(load(&key_ref(service)).await, Ok(Some(_)))
}
