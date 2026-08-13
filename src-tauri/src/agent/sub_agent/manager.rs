use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use super::config::SubAgentConfig;
use super::db::{SubAgentDb, SubAgentRecord, SubAgentRunTraceRecord};

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
            // 单条解析失败直接向上抛错：不得用「部分成功」结果覆盖缓存，
            // 否则该代理会在内存视图中静默消失，而 DB/list_all 仍可见，
            // 造成前后端行为分叉。降级策略由调用方决定。
            let mut config = serde_json::from_str::<SubAgentConfig>(&record.config_json)
                .with_context(|| format!("子智能体 '{}' 配置解析失败", record.id))?;
            config.enabled = record.enabled;
            new_cache.insert(config.agent_id.clone(), config.clone());
            configs.push(config);
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
        // DB 只按主键 id 更新行，id 列本身不会被改写。若允许 config.agent_id
        // 与 id 不一致，缓存会插入新键而旧键残留，导致缓存与 DB 永久分叉。
        if config.agent_id != id {
            anyhow::bail!(
                "错误：子智能体 id 不允许变更（期望 '{id}'，配置中为 '{}'）",
                config.agent_id
            );
        }
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

    pub fn get_enabled_for_session(&self, session_id: &str) -> Result<Vec<SubAgentConfig>> {
        let ids = self.db.get_enabled_agent_ids(session_id)?;
        let cache = self.cache.read();
        Ok(ids.iter().filter_map(|id| cache.get(id).cloned()).collect())
    }

    pub fn get_global_enabled(&self) -> Result<Vec<SubAgentRecord>> {
        self.db.get_global_enabled()
    }

    pub fn set_global_enabled(&self, sub_agent_ids: &[String]) -> Result<()> {
        self.db.set_global_enabled(sub_agent_ids)?;
        // 与 create/update/delete/seed_browser_force 保持一致：写库后刷新缓存，
        // 避免 cache 与 DB 的使能状态长期不一致。
        self.load_all()?;
        Ok(())
    }

    pub fn save_run_trace(
        &self,
        workspace_id: &str,
        tool_call_id: &str,
        agent_id: &str,
        status: &str,
        events_json: &str,
    ) -> Result<SubAgentRunTraceRecord> {
        self.db
            .save_run_trace(workspace_id, tool_call_id, agent_id, status, events_json)
    }

    pub fn get_run_trace(
        &self,
        workspace_id: &str,
        tool_call_id: &str,
    ) -> Result<Option<SubAgentRunTraceRecord>> {
        self.db.get_run_trace(workspace_id, tool_call_id)
    }
}
