use serde::{Serialize, de::DeserializeOwned};
use tracing::warn;
use std::borrow::Borrow;
use std::hash::Hash;
use std::{collections::HashMap, fs::{self, OpenOptions}, path::PathBuf, sync::{Arc, RwLock}};
use directories::ProjectDirs;
use miette::{IntoDiagnostic, Result};

use crate::{APPLICATION, ORGANIZATION, QUALIFIER};

#[derive(Debug)]
pub struct Cache<K, V> {
    pub map: HashMap<K, V>,
    path: PathBuf,
}

impl<K, V> Cache<K, V>
where
    K: Serialize + DeserializeOwned + Eq + Hash,
    V: Serialize + DeserializeOwned,
{
    
    pub fn load(name: &str) -> Result<Self> {
        let base = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).ok_or(miette::miette!("No project dirs"))?;
        let cache_dir = base.cache_dir();

        // Ensure the cache dir exists
        fs::create_dir_all(cache_dir).into_diagnostic()?;

        let path = cache_dir.join(format!("{}.json", name));

        // Return a fresh cache if the file doesn't exist
        if !path.exists() {
            return Ok(Self { map: HashMap::new(), path });
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .into_diagnostic()?;

        let map = match serde_json::from_reader(file) {
            Ok(m) => m,
            Err(e) => {
                warn!(path=path.to_str(), error=e.to_string(), "{} cache file was invalid, overwriting.", name);
                HashMap::new()
            }
        };

        Ok(Self { map, path })
    }

    pub fn save(&self) -> Result<()> {
        let text = serde_json::to_string_pretty(&self.map).into_diagnostic()?;
        fs::write(&self.path, text).into_diagnostic()?;
        Ok(())
    }

    pub fn into_threadsafe(self) -> Arc<RwLock<Cache<K, V>>> {
        Arc::new(RwLock::new(self))
    }

    // Convenience wrappers
    pub fn insert(&mut self, key: K, value: V) { self.map.insert(key, value); }
    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.map.get(key)
    }
}

pub type AsyncCache<K, V> = Arc<RwLock<Cache<K, V>>>;
pub type AsyncZoneCache = AsyncCache<String, String>;

