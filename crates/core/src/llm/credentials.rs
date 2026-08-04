use keyring::Entry;

const SERVICE_NAME: &str = "post-office-llm";

pub fn store_provider_api_key(provider_id: &str, api_key: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE_NAME, provider_id).map_err(|error| error.to_string())?;
    if api_key.trim().is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    } else {
        entry
            .set_password(api_key)
            .map_err(|error| error.to_string())
    }
}

pub fn load_provider_api_key(provider_id: &str) -> Result<Option<String>, String> {
    let entry = Entry::new(SERVICE_NAME, provider_id).map_err(|error| error.to_string())?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
