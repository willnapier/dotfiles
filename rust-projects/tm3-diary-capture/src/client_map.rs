use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct ClientMapFile {
    clients: HashMap<String, String>,
}

pub struct ClientMap {
    map: HashMap<String, String>,
}

impl ClientMap {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read client map: {}", path.display()))?;
        let file: ClientMapFile =
            toml::from_str(&content).context("Failed to parse client map TOML")?;
        Ok(Self { map: file.clients })
    }

    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .expect("Could not find home directory")
            .join("Clinical/private/tm3-client-map.toml")
    }

    pub fn lookup(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_load_and_lookup() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(
            tmp,
            r#"[clients]
"Smith, Jane" = "JS92"
"Jones, Bob and Alice" = "BJ+AJ"
"#
        )
        .unwrap();

        let map = ClientMap::load(tmp.path()).unwrap();
        assert_eq!(map.lookup("Smith, Jane"), Some("JS92"));
        assert_eq!(map.lookup("Jones, Bob and Alice"), Some("BJ+AJ"));
        assert_eq!(map.lookup("Unknown, Person"), None);
    }
}
