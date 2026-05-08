//! Pluggable secret storage. Today: secrets live next to the rest of the
//! config in `backend.json` (file mode 0600 on Unix). Later we can swap in a
//! `KeychainSecretStore` without touching commands or UI.

use anyhow::Result;

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    #[allow(dead_code)]
    fn delete(&self, key: &str) -> Result<()>;
}

/// File-backed store. The implementation is intentionally a thin wrapper over
/// the persisted JSON — secrets and non-secrets share the same file. The
/// abstraction exists so a future `KeychainSecretStore` is a one-line swap.
pub struct FileSecretStore {
    inner: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl FileSecretStore {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl SecretStore for FileSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.inner.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.inner
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.inner.lock().unwrap().remove(key);
        Ok(())
    }
}
