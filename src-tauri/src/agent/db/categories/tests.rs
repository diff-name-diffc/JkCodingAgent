use super::*;

fn test_db() -> (DispatcherDb, crate::test_util::TempDirGuard) {
    let dir = crate::test_util::TempDirGuard::new("aha-category-config");
    (
        DispatcherDb::new(dir.path().join("jkbot.sqlite3")).expect("create test dispatcher db"),
        dir,
    )
}

#[test]
fn init_backfills_default_category_configs() {
    let (db, _dir) = test_db();

    let categories = db.list_chat_categories().expect("list categories");
    let configs = db
        .list_chat_category_agent_configs()
        .expect("list category configs");

    assert!(!categories.is_empty());
    assert_eq!(categories.len(), configs.len());
    assert!(configs
        .iter()
        .all(|config| !config.system_prompt.trim().is_empty()));
    let tech = configs
        .iter()
        .find(|config| config.category_id == "tech")
        .expect("tech config exists");
    let life = configs
        .iter()
        .find(|config| config.category_id == "life")
        .expect("life config exists");
    assert_ne!(tech.system_prompt, life.system_prompt);
    assert!(tech.allowed_tools.contains(&"local_zsh".to_string()));
    assert!(!life.allowed_tools.contains(&"local_zsh".to_string()));
}

#[test]
fn listing_configs_repairs_missing_rows() {
    let (db, _dir) = test_db();
    db.conn()
        .expect("db conn")
        .execute(
            "DELETE FROM chat_category_agent_configs WHERE category_id = 'tech'",
            [],
        )
        .expect("delete one config");

    let configs = db
        .list_chat_category_agent_configs()
        .expect("list category configs");

    assert!(configs.iter().any(|config| config.category_id == "tech"));
}

#[test]
fn corrupt_config_json_degrades_on_read_and_self_heals_on_save() {
    let (db, _dir) = test_db();
    db.conn()
        .expect("db conn")
        .execute(
            "UPDATE chat_category_agent_configs
             SET allowed_tools_json = '{not valid json', sub_agent_ids_json = 'oops'
             WHERE category_id = 'tech'",
            [],
        )
        .expect("corrupt one config row");

    // 读取不得因单行坏数据整体失败：降级为空列表。
    let configs = db
        .list_chat_category_agent_configs()
        .expect("list category configs despite corrupt row");
    let tech = configs
        .iter()
        .find(|config| config.category_id == "tech")
        .expect("tech config still readable");
    assert!(tech.allowed_tools.is_empty(), "坏 JSON 应降级为空列表");
    assert!(tech.sub_agent_ids.is_empty(), "坏 JSON 应降级为空列表");

    // 保存路径应自愈坏行：重写为默认值。
    let mut repaired = configs.clone();
    let tech_config = repaired
        .iter_mut()
        .find(|config| config.category_id == "tech")
        .expect("tech config to save");
    tech_config.system_prompt = "repaired prompt".to_string();
    let saved = db
        .save_chat_category_agent_configs(&repaired)
        .expect("save category configs after corruption");
    let tech_saved = saved
        .iter()
        .find(|config| config.category_id == "tech")
        .expect("tech config after save");
    assert_eq!(tech_saved.system_prompt, "repaired prompt");

    // 再次读取确认坏行已被修复为可解析的默认工具列表。
    let raw_tools: String = db
        .conn()
        .expect("db conn")
        .query_row(
            "SELECT allowed_tools_json FROM chat_category_agent_configs WHERE category_id = 'tech'",
            [],
            |row| row.get(0),
        )
        .expect("read repaired json");
    assert!(
        serde_json::from_str::<Vec<String>>(&raw_tools).is_ok(),
        "保存后坏行必须被修复为合法 JSON"
    );
}

#[test]
fn deleting_missing_category_reports_error() {
    let (db, _dir) = test_db();
    let error = db
        .delete_chat_category("no-such-category")
        .expect_err("deleting a missing category must fail");
    assert!(error.to_string().contains("聊天分类不存在"));
}

#[test]
fn deleting_existing_category_succeeds() {
    let (db, _dir) = test_db();
    let category = db
        .create_chat_category("待删除", "Folder", "#222222", None, None)
        .expect("create category");
    db.delete_chat_category(&category.id)
        .expect("delete existing category");
    let remaining = db.list_chat_categories().expect("list categories");
    assert!(remaining.iter().all(|item| item.id != category.id));
}

#[test]
fn creating_category_creates_agent_config() {
    let (db, _dir) = test_db();
    let category = db
        .create_chat_category("测试", "Folder", "#111111", None, None)
        .expect("create category");

    let configs = db
        .list_chat_category_agent_configs()
        .expect("list category configs");

    assert!(configs
        .iter()
        .any(|config| config.category_id == category.id));
}

#[test]
fn creating_category_can_override_agent_config() {
    let (db, _dir) = test_db();
    let category = db
        .create_chat_category(
            "自定义",
            "Folder",
            "#111111",
            Some(vec!["browser_open_url".to_string()]),
            Some("custom prompt"),
        )
        .expect("create category");

    let configs = db
        .list_chat_category_agent_configs()
        .expect("list category configs");
    let config = configs
        .iter()
        .find(|config| config.category_id == category.id)
        .expect("new category config");

    assert_eq!(config.system_prompt, "custom prompt");
    assert_eq!(config.allowed_tools, vec!["browser_open_url".to_string()]);
}

#[test]
fn session_resolves_its_category_agent_config() {
    let (db, _dir) = test_db();
    let configs = db
        .save_chat_category_agent_configs(&[ChatCategoryAgentConfig {
            category_id: "tech".to_string(),
            category_name: "技术".to_string(),
            allowed_tools: vec!["browser_open_url".to_string()],
            system_prompt: "tech prompt".to_string(),
            sub_agent_ids: vec!["browser-agent".to_string()],
            project_directories: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }])
        .expect("save category configs");
    assert!(configs.iter().any(|config| config.category_id == "tech"));

    let session = db
        .create_chat_session("技术会话", Some("tech"))
        .expect("create chat session");
    let config = db
        .get_chat_session_category_agent_config(&session.id)
        .expect("get session category config")
        .expect("session has category config");

    assert_eq!(config.category_id, "tech");
    assert_eq!(config.system_prompt, "tech prompt");
    assert_eq!(config.allowed_tools, vec!["browser_open_url".to_string()]);
    assert_eq!(config.sub_agent_ids, vec!["browser-agent".to_string()]);
}

#[test]
fn category_directories_are_normalized_and_shared_by_its_chat_sessions() {
    let (db, dir) = test_db();
    let project = dir.path().join("project");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&artifacts).unwrap();
    let mut config = db
        .list_chat_category_agent_configs()
        .unwrap()
        .into_iter()
        .find(|config| config.category_id == "tech")
        .unwrap();
    config.project_directories = vec![
        project.display().to_string(),
        artifacts.display().to_string(),
        project.join(".").display().to_string(),
    ];
    db.save_chat_category_agent_configs(&[config]).unwrap();
    let expected = [project, artifacts]
        .iter()
        .map(|path| path.canonicalize().unwrap().display().to_string())
        .collect::<Vec<_>>();
    for title in ["会话一", "会话二"] {
        let session = db.create_chat_session(title, Some("tech")).unwrap();
        let config = db
            .get_chat_session_category_agent_config(&session.id)
            .unwrap()
            .unwrap();
        assert_eq!(config.project_directories, expected);
    }
    let project_session = db
        .create_project_session("unrelated-project", "项目会话")
        .unwrap();
    assert!(db
        .get_chat_session_category_agent_config(&project_session.id)
        .unwrap()
        .is_none());
}

#[test]
fn invalid_directories_reject_the_whole_settings_batch_before_writing() {
    let (db, _dir) = test_db();
    let mut configs = db.list_chat_category_agent_configs().unwrap();
    let original_prompt = configs[0].system_prompt.clone();
    configs[0].system_prompt = "不应保存".to_string();
    configs[1].project_directories = vec!["relative/project".to_string()];
    let error = db.save_chat_category_agent_configs(&configs).unwrap_err();
    assert!(format!("{error:#}").contains("绝对路径"));
    let saved = db.list_chat_category_agent_configs().unwrap();
    assert_eq!(saved[0].system_prompt, original_prompt);
    assert!(saved
        .iter()
        .all(|config| config.project_directories.is_empty()));
}

#[test]
fn corrupted_directory_json_fails_loudly_and_is_not_repaired_to_empty() {
    let (db, _dir) = test_db();
    let mut other_config = db
        .list_chat_category_agent_configs()
        .unwrap()
        .into_iter()
        .find(|config| config.category_id == "life")
        .unwrap();
    let original_prompt = other_config.system_prompt.clone();
    db.conn()
        .unwrap()
        .execute(
            "UPDATE chat_category_agent_configs SET project_directories_json = '{broken',
             allowed_tools_json = 'broken' WHERE category_id = 'tech'",
            [],
        )
        .unwrap();
    let error = db.list_chat_category_agent_configs().unwrap_err();
    assert!(format!("{error:#}").contains("tech 的共享工程目录配置 JSON 损坏"));
    let session = db.create_chat_session("损坏目录", Some("tech")).unwrap();
    assert!(db
        .get_chat_session_category_agent_config(&session.id)
        .is_err());
    other_config.system_prompt = "失败的更新".to_string();
    assert!(db
        .save_chat_category_agent_configs(&[other_config])
        .is_err());
    let conn = db.conn().unwrap();
    let corrupted: String = conn
        .query_row(
            "SELECT project_directories_json FROM chat_category_agent_configs WHERE category_id = 'tech'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(corrupted, "{broken");
    let prompt: String = conn
        .query_row(
            "SELECT system_prompt FROM chat_category_agent_configs WHERE category_id = 'life'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(prompt, original_prompt, "读取错误必须回滚整个保存事务");
}
