use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use parking_lot::RwLock;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use super::config::SubAgentConfig;
use super::db::{SubAgentDb, SubAgentRecord};

pub struct SubAgentManager {
    db: SubAgentDb,
    cache: RwLock<HashMap<String, SubAgentConfig>>,
}

impl SubAgentManager {
    pub fn new(pool: Arc<Pool<SqliteConnectionManager>>) -> Self {
        Self {
            db: SubAgentDb::new(pool),
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub fn load_all(&self) -> Result<Vec<SubAgentConfig>> {
        let records = self.db.list_all()?;
        let mut configs = Vec::new();
        let mut new_cache = HashMap::new();
        for record in records {
            match serde_json::from_str::<SubAgentConfig>(&record.config_json) {
                Ok(mut config) => {
                    config.enabled = record.enabled;
                    new_cache.insert(config.agent_id.clone(), config.clone());
                    configs.push(config);
                }
                Err(e) => {
                    eprintln!(
                        "SubAgentManager: failed to parse config for '{}': {}",
                        record.id, e
                    );
                }
            }
        }
        *self.cache.write() = new_cache;
        Ok(configs)
    }

    pub fn get(&self, agent_id: &str) -> Option<SubAgentConfig> {
        self.cache.read().get(agent_id).cloned()
    }

    pub fn create(&self, config: SubAgentConfig) -> Result<SubAgentRecord> {
        config.validate()?;
        let record = self.db.create(&config)?;
        self.cache.write().insert(config.agent_id.clone(), config);
        Ok(record)
    }

    pub fn update(&self, id: &str, config: SubAgentConfig) -> Result<SubAgentRecord> {
        config.validate()?;
        let record = self.db.update(id, &config)?;
        self.cache.write().insert(config.agent_id.clone(), config);
        Ok(record)
    }

    pub fn delete(&self, agent_id: &str) -> Result<()> {
        self.db.delete(agent_id)?;
        self.cache.write().remove(agent_id);
        Ok(())
    }

    pub fn seed_browser_force(&self) -> Result<()> {
        self.db.seed_browser_force()?;
        self.load_all()?;
        Ok(())
    }

    pub fn list_all(&self) -> Result<Vec<SubAgentRecord>> {
        self.db.list_all()
    }

    pub fn get_record(&self, id: &str) -> Result<Option<SubAgentRecord>> {
        self.db.get(id)
    }

    pub fn get_enabled_for_session(&self, session_id: &str) -> Result<Vec<SubAgentConfig>> {
        let ids = self.db.get_enabled_agent_ids(session_id)?;
        let cache = self.cache.read();
        Ok(ids.iter().filter_map(|id| cache.get(id).cloned()).collect())
    }

    pub fn get_global_enabled(&self) -> Result<Vec<SubAgentRecord>> {
        self.db.get_global_enabled()
    }

    pub fn set_global_enabled(&self, sub_agent_ids: &[String]) -> Result<()> {
        self.db.set_global_enabled(sub_agent_ids)
    }
}
