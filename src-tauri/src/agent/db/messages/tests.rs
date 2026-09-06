use rusqlite::params;

use super::DispatcherDb;

fn test_db() -> DispatcherDb {
    let path = std::env::temp_dir().join(format!(
        "jkcodingagent-messages-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    DispatcherDb::new(path).expect("create test dispatcher db")
}

fn add_text_message(db: &DispatcherDb, session_id: &str, role: &str, text: &str) {
    let segments_json = super::super::content::content_to_segments_json(text);
    db.add_visible_message_from_segments(session_id, role, segments_json)
        .expect("add message");
}

#[test]
fn count_visible_messages_is_session_scoped() {
    let db = test_db();
    let session = db
        .create_chat_session("messages", Some("tech"))
        .expect("create chat session");
    assert_eq!(db.count_visible_messages(&session.id).expect("count"), 0);

    add_text_message(&db, &session.id, "user", "你好");
    add_text_message(&db, &session.id, "assistant", "有什么可以帮你？");
    assert_eq!(db.count_visible_messages(&session.id).expect("count"), 2);

    // 会话隔离：其他会话的消息不计入本会话计数。
    let other = db
        .create_chat_session("other", Some("tech"))
        .expect("create other chat session");
    add_text_message(&db, &other.id, "user", "另一会话的消息");
    assert_eq!(db.count_visible_messages(&session.id).expect("count"), 2);
    assert_eq!(db.count_visible_messages(&other.id).expect("count"), 1);
}

#[test]
fn chat_image_registration_is_session_scoped_and_rebindable() {
    let db = test_db();
    let first = db
        .create_chat_session("first", Some("tech"))
        .expect("create first session");
    let second = db
        .create_chat_session("second", Some("tech"))
        .expect("create second session");

    let registration =
        |image_id: &str, workspace_id: &str| crate::chat_images::ChatImageRegistration {
            image_id: image_id.to_string(),
            workspace_id: workspace_id.to_string(),
            width: None,
            height: None,
            mime_type: "image/png".to_string(),
            source: "user_paste".to_string(),
            generation_prompt: None,
        };

    // 保存即登记：message_id 为 NULL 的索引行，按 workspace 隔离。
    db.register_chat_image(
        &registration("image-a", &first.id),
        std::path::Path::new("/tmp/a.png"),
    )
    .expect("register first image");
    db.register_chat_image(
        &registration("image-b", &second.id),
        std::path::Path::new("/tmp/b.png"),
    )
    .expect("register second image");

    let count_for = |workspace_id: &str| -> i64 {
        db.conn()
            .expect("db conn")
            .query_row(
                "SELECT COUNT(*) FROM chat_images WHERE workspace_id = ?1",
                params![workspace_id],
                |row| row.get(0),
            )
            .expect("count chat images")
    };
    assert_eq!(count_for(&first.id), 1);
    assert_eq!(count_for(&second.id), 1);

    // 同 image_id 换会话重新登记时整体重绑（UPSERT 不再静默跳过）。
    db.register_chat_image(
        &registration("image-a", &second.id),
        std::path::Path::new("/tmp/a.png"),
    )
    .expect("rebind image to second session");
    assert_eq!(count_for(&first.id), 0);
    assert_eq!(count_for(&second.id), 2);
}

#[test]
fn chat_image_rows_cascade_with_message_deletion() {
    let db = test_db();
    let session = db
        .create_chat_session("session", Some("tech"))
        .expect("create session");
    let message = db
        .add_visible_message_from_segments(
            &session.id,
            "user",
            super::super::content::content_to_segments_json("带图消息"),
        )
        .expect("add message");

    db.conn()
        .expect("db conn")
        .execute(
            "INSERT INTO chat_images (
                    id, image_id, workspace_id, message_id, segment_index, path, created_at
                 ) VALUES ('row-1', 'image-1', ?1, ?2, 0, '/tmp/first.png', '2026-01-01T00:00:00Z')",
            params![session.id, message.id],
        )
        .expect("insert message-bound chat image");

    // 消息删除时索引行随外键级联消失（连接池逐连接 foreign_keys=ON）。
    db.conn()
        .expect("db conn")
        .execute(
            "DELETE FROM dispatcher_messages WHERE id = ?1",
            params![message.id],
        )
        .expect("delete message");
    let remaining: i64 = db
        .conn()
        .expect("db conn")
        .query_row(
            "SELECT COUNT(*) FROM chat_images WHERE workspace_id = ?1",
            params![session.id],
            |row| row.get(0),
        )
        .expect("count chat images");
    assert_eq!(remaining, 0);
}

#[test]
fn tool_context_payload_is_serialized_and_matches_llm_input() {
    let db = test_db();
    let message = db
        .add_visible_tool_result(
            "workspace",
            "给用户看的短摘要",
            "frame 37: 图号 A-01，标题 总说明\nframe 38: 图号 A-02，标题 系统图",
            Some("tool-call"),
            Some("read_dwg"),
            Some("intent_compressed"),
            &[],
        )
        .expect("add compressed tool result");

    let serialized = serde_json::to_value(&message).expect("serialize dispatcher message");
    assert_eq!(
        serialized["contextPayload"],
        "frame 37: 图号 A-01，标题 总说明\nframe 38: 图号 A-02，标题 系统图"
    );
    assert_eq!(
        message.to_llm_message().expect("tool message kept").content,
        serialized["contextPayload"].as_str().unwrap()
    );
}

fn image_segment(image_id: &str) -> super::super::content::ContentSegment {
    super::super::content::ContentSegment::Image {
        id: uuid::Uuid::new_v4().to_string(),
        image_id: image_id.to_string(),
        alt: None,
        width: None,
        height: None,
        mime_type: Some("image/png".to_string()),
        source: "user_paste".to_string(),
        generation_prompt: None,
    }
}

#[test]
fn user_image_segments_expose_resolvable_chat_image_reference() {
    use crate::agent::llm::{ChatMessageContentPart, ChatMessageImageSource};

    let segments = vec![
        image_segment("563ff830-d978-4c77-b1ec-ba8b620752aa"),
        super::super::content::ContentSegment::Text {
            id: uuid::Uuid::new_v4().to_string(),
            text: "分析一下这个图片".to_string(),
        },
    ];

    let parts = super::segments_to_llm_content_parts("user", &segments);

    // 图片部分之后紧跟引用文本标注，模型可见且可原样复制。
    assert_eq!(parts.len(), 3);
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::Image {
            source: ChatMessageImageSource::ChatImage { image_id }
        } if image_id == "563ff830-d978-4c77-b1ec-ba8b620752aa"
    ));
    let ChatMessageContentPart::Text { text } = &parts[1] else {
        panic!("图片之后应为引用文本标注");
    };
    assert!(text.contains("chat-image://563ff830-d978-4c77-b1ec-ba8b620752aa"));
    assert!(matches!(&parts[2], ChatMessageContentPart::Text { .. }));
}

#[test]
fn image_reference_note_is_only_added_for_user_role() {
    let segments = vec![image_segment("some-image-id")];

    assert!(super::segments_to_llm_content_parts("assistant", &segments).is_empty());
    assert!(super::segments_to_llm_content_parts("tool", &segments).is_empty());
    assert_eq!(
        super::segments_to_llm_content_parts("user", &segments).len(),
        2
    );
}

// ── truncate_messages_from（regenerate / 编辑重发共用截断）─────────────

#[test]
fn truncate_messages_from_cleans_traces_and_graph_plans() {
    let db = test_db();
    let session = db
        .create_chat_session("truncate", Some("tech"))
        .expect("create session");
    let conn = db.conn().expect("db conn");

    let segments_json = |text: &str| super::super::content::content_to_segments_json(text);
    let m1 = db
        .add_visible_message_from_segments(&session.id, "user", segments_json("第一轮提问"))
        .expect("add m1");
    let m2 = db
        .add_visible_message_from_segments(&session.id, "assistant", segments_json("第一轮回答"))
        .expect("add m2");
    let m3 = db
        .add_visible_message_from_segments(&session.id, "user", segments_json("被编辑的用户消息"))
        .expect("add m3");
    let m4 = db
        .add_visible_message_from_segments(&session.id, "assistant", segments_json("将被删除的回答"))
        .expect("add m4");
    let target_ms = chrono::DateTime::parse_from_rfc3339(&m3.created_at)
        .expect("parse m3 created_at")
        .timestamp_millis();

    // 工具运行：call-kept 绑定截断点之前的 m2（应保留）；call-gone 绑定被删的
    // m4；call-pending 为 message_id NULL 的运行中记录（created_at >= 目标时刻）。
    let insert_run = |id: &str, call_id: &str, message_id: Option<&str>, created_at: &str| {
        conn.execute(
            "INSERT INTO dispatcher_tool_runs (
                 id, workspace_id, tool_call_id, tool_name, provider, category, status,
                 message_id, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'run_sub_agent', 'builtin', 'agent', 'completed', ?4, ?5, ?5)",
            params![id, session.id, call_id, message_id, created_at],
        )
        .expect("insert tool run");
    };
    insert_run("run-kept", "call-kept", Some(&m2.id), &m2.created_at);
    insert_run("run-gone", "call-gone", Some(&m4.id), &m4.created_at);
    insert_run("run-pending", "call-pending", None, &m3.created_at);

    let insert_trace = |call_id: &str| {
        conn.execute(
            "INSERT INTO sub_agent_run_traces (
                 workspace_id, tool_call_id, agent_id, status, events_json, created_at, updated_at
             ) VALUES (?1, ?2, 'browser-agent', 'completed', '[]', ?3, ?3)",
            params![session.id, call_id, m2.created_at],
        )
        .expect("insert sub-agent trace");
    };
    insert_trace("call-kept");
    insert_trace("call-gone");
    insert_trace("call-pending");

    // 图编排：新计划创建于目标消息之后（应随截断删除，级联 run/node_run/activity）；
    // 旧计划早于目标消息一小时（应保留）。
    let insert_plan = |id: &str, created_at_ms: i64| {
        conn.execute(
            "INSERT INTO graph_plans (
                 id, workspace_id, title, definition_json, created_at, updated_at
             ) VALUES (?1, ?2, 'plan', '{}', ?3, ?3)",
            params![id, session.id, created_at_ms],
        )
        .expect("insert graph plan");
    };
    insert_plan("plan-old", target_ms - 3_600_000);
    insert_plan("plan-new", target_ms + 1_000);
    conn.execute(
        "INSERT INTO graph_runs (id, plan_id, attempt_no, status, started_at)
         VALUES ('run-1', 'plan-new', 1, 'succeeded', ?1)",
        params![target_ms],
    )
    .expect("insert graph run");
    conn.execute(
        "INSERT INTO graph_node_runs (
             run_id, plan_id, node_id, model_ref, model_label, model_category, base_tool_group
         ) VALUES ('run-1', 'plan-new', 'node-1', 'ref', 'label', 'chat', 'coder')",
        [],
    )
    .expect("insert graph node run");
    conn.execute(
        "INSERT INTO graph_node_activities (id, run_id, node_id, sequence, kind, status, started_at)
         VALUES ('act-1', 'run-1', 'node-1', 0, 'log', 'completed', ?1)",
        params![target_ms],
    )
    .expect("insert graph node activity");

    // python 运行记录绑定被删消息，应随外键级联消失。
    conn.execute(
        "INSERT INTO python_code_runs (
             run_id, workspace_id, message_id, code_block_index, code_hash, code, status,
             created_at, updated_at
         ) VALUES ('py-1', ?1, ?2, 0, 'hash', 'print(1)', 'completed', ?3, ?3)",
        params![session.id, m4.id, m4.created_at],
    )
    .expect("insert python run");

    // token 用量为真实消耗记录，截断不回退。
    conn.execute(
        "INSERT INTO dispatcher_session_token_usage (workspace_id, model, updated_at)
         VALUES (?1, 'test-model', ?2)",
        params![session.id, m2.created_at],
    )
    .expect("insert token usage");

    let removed = db
        .truncate_messages_from(&session.id, &m3.id)
        .expect("truncate");
    assert_eq!(removed, 2);

    let count = |sql: &str, id: &str| -> i64 {
        conn.query_row(sql, params![id], |row| row.get(0))
            .expect("count query")
    };
    // 消息：目标及之后删除，之前保留。
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM dispatcher_messages WHERE workspace_id = ?1",
            &session.id
        ),
        2
    );
    assert!(conn
        .query_row(
            "SELECT COUNT(*) FROM dispatcher_messages WHERE id = ?1",
            params![m1.id],
            |row| row.get::<_, i64>(0)
        )
        .expect("m1 kept")
        == 1);
    // 工具运行与产物：截断范围内的删除，范围外的保留。
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM dispatcher_tool_runs WHERE workspace_id = ?1",
            &session.id
        ),
        1
    );
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM dispatcher_tool_runs WHERE tool_call_id = 'call-kept' AND workspace_id = ?1",
            &session.id
        ),
        1
    );
    // 子智能体 trace：按被删 tool_call_id 精确回收，call-kept 不受牵连。
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM sub_agent_run_traces WHERE workspace_id = ?1",
            &session.id
        ),
        1
    );
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM sub_agent_run_traces WHERE tool_call_id = 'call-kept' AND workspace_id = ?1",
            &session.id
        ),
        1
    );
    // 图编排：新计划连同 run/node_run/activity 级联删除，旧计划保留。
    assert_eq!(
        count("SELECT COUNT(*) FROM graph_plans WHERE workspace_id = ?1", &session.id),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM graph_plans WHERE id = 'plan-old' AND workspace_id = ?1", &session.id),
        1
    );
    assert_eq!(count("SELECT COUNT(*) FROM graph_runs WHERE plan_id = ?1", "plan-new"), 0);
    assert_eq!(
        count("SELECT COUNT(*) FROM graph_node_runs WHERE plan_id = ?1", "plan-new"),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM graph_node_activities WHERE run_id = ?1", "run-1"),
        0
    );
    // python 运行记录随消息外键级联删除。
    assert_eq!(
        count("SELECT COUNT(*) FROM python_code_runs WHERE workspace_id = ?1", &session.id),
        0
    );
    // token 用量有意保留（真实消耗记录）。
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            &session.id
        ),
        1
    );
}
