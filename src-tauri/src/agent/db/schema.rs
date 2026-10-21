//! 数据库 schema 初始化与版本管理（PRAGMA user_version 方案）。
//!
//! 当前版本 v18，保留 v16 工作流基线并前向迁移浏览器配置与聊天分类共享目录。
//! 全新库直接创建当前形态；v16 库先备份再事务迁移；低于 v16 的旧开发库
//! 没有迁移路径，仍拒绝打开。旧链重用过版本号，同版本也必须校验领域形态。
//!
//! ## 后续 schema 变更规范（详见 AGENTS.md「存储 schema 迁移」）
//!
//! 1. 更新本文件的基线 DDL，使新装库直接得到新形态；
//! 2. 递增 `SCHEMA_VERSION`，并在 `init()` 的迁移挂载点追加
//!    `if current_version < N` 的事务块（DDL/回填 + `user_version` 推进同事务）；
//! 3. 迁移块必须幂等、可重试；破坏性变更（DROP/清空）前先做整库快照备份。

use super::DispatcherDb;
use anyhow::{Context, Result};

/// 当前 schema 版本号（PRAGMA user_version）。
///
/// v16：工作流口径基线；v17：browser-use 专用运行时与退役工具配置迁移。
/// v18：普通聊天分类可绑定多个共享工程目录。
pub(crate) const SCHEMA_VERSION: i32 = 18;

mod baseline;
mod categories;
mod migration_v17;
mod migration_v18;
mod runtime;
mod v16_prompts;

#[cfg(test)]
mod tests;

use baseline::create_baseline;
pub(super) use categories::{
    backfill_chat_category_agent_configs_tx, default_chat_category_agent_config_tx,
};

impl DispatcherDb {
    pub(super) fn init(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        // 连接池 with_init 已逐连接设置 WAL / busy_timeout / foreign_keys=ON，
        // 这里无需重复声明 PRAGMA。
        let mut conn = self.conn()?;

        // 读取失败说明数据库文件损坏 / IO 异常：不能按 0（全新库）处理，
        // 否则会在损坏文件上继续建表，掩盖真实故障。
        let current_version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .context("read PRAGMA user_version（数据库可能已损坏）")?;

        if current_version == SCHEMA_VERSION {
            return migration_v18::verify_shape(&conn);
        }
        if current_version > SCHEMA_VERSION {
            // 版本号更高有两种来源：应用降级打开了新版写入的库，或（当前开发阶段
            // 的实际情况）schema 基线重置前的旧开发库——其 user_version 落在 2..=33，
            // 迁移链已清除，无法识别也无法迁移。两种情况都不能继续读写：轻则查询
            // 报错、重则写入不兼容数据损坏库。
            anyhow::bail!(
                "数据库版本({current_version})与当前基线版本({SCHEMA_VERSION})不兼容。\
                 若为 schema 重置前的旧开发库，请退出应用后删除数据库文件 {}（连同 -wal/-shm 残留文件）再启动；\
                 否则请使用写入该数据库的更新版本应用打开。",
                self.path.display()
            );
        }
        if current_version == 0 {
            // user_version=0 但库里已有任何表 → 迁移链清除前的旧开发库（v1 之前
            // 的版本号都大于 0，正常旧库会命中上方的版本比较分支）。检查「任何表」
            // 而非单看 dispatcher_sessions：残缺旧库可能缺核心表但残留其它表，
            // 若误判为全新库，CREATE TABLE IF NOT EXISTS 不会清理残留，会形成
            // 新旧 schema 混合。
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get(0),
                )
                .context("inspect sqlite_master（数据库可能已损坏）")?;
            if table_count > 0 {
                anyhow::bail!(
                    "检测到 schema 重置前的旧开发数据库（user_version=0 但已有数据表），无法自动迁移。\
                     请退出应用后删除数据库文件 {}（连同 -wal/-shm 残留文件）再启动。",
                    self.path.display()
                );
            }
            create_baseline(&mut conn)?;
            return Ok(());
        }

        if current_version < 16 {
            anyhow::bail!(
                "数据库版本({current_version})低于当前基线版本({SCHEMA_VERSION})且已无迁移路径；\
             请退出应用后删除数据库文件 {}（连同 -wal/-shm 残留文件）再启动。",
                self.path.display()
            );
        }

        if current_version < 17 {
            migration_v17::verify_shape(&conn)?;
            migration_v17::backup(&conn, &self.path)?;
            let tx = conn.transaction().context("begin schema v17 migration")?;
            let disabled = migration_v17::migrate(&tx)?;
            tx.pragma_update(None, "user_version", 17)?;
            tx.commit().context("commit schema v17 migration")?;
            for id in disabled {
                eprintln!("子智能体 {id} 的旧浏览器工具已退役，当前工具列表为空；已禁用并保留配置，请重新选择工具后启用。");
            }
        }
        if current_version < 18 {
            migration_v17::verify_shape(&conn)?;
            let tx = conn.transaction().context("begin schema v18 migration")?;
            migration_v18::migrate(&tx)?;
            tx.pragma_update(None, "user_version", 18)?;
            tx.commit().context("commit schema v18 migration")?;
        }
        Ok(())
    }
}
