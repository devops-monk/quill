//! `search_schema` against a real SQLite database: metadata loading, FK
//! discovery, schema linking and rendering, end to end through the agent tool
//! dispatcher.

use dbx_core::agent_events::ToolCall;
use dbx_core::ai::agent_tools::{execute_tool, AgentSqlPermissions};
use dbx_core::connection::AppState;
use dbx_core::models::connection::{ConnectionConfig, DatabaseType};
use serde_json::json;
use std::sync::Arc;

const SHOP_DDL: &str = "\
    CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, country TEXT); \
    CREATE TABLE products (id INTEGER PRIMARY KEY, title TEXT NOT NULL, unit_price REAL); \
    CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL REFERENCES customers(id), \
        total_amount REAL, created_at TEXT); \
    CREATE TABLE order_items (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL REFERENCES orders(id), \
        product_id INTEGER NOT NULL REFERENCES products(id), quantity INTEGER);";

fn sqlite_config(id: &str, path: &str) -> ConnectionConfig {
    let mut config: ConnectionConfig = serde_json::from_value(json!({
        "id": id,
        "name": id,
        "db_type": "sqlite",
        "host": path,
        "port": 0,
        "username": "",
        "password": "",
    }))
    .expect("minimal sqlite config deserializes with defaults");
    config.db_type = DatabaseType::Sqlite;
    config
}

async fn shop_state(name: &str, filler_tables: usize) -> (Arc<AppState>, String) {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let dir = std::env::temp_dir().join(format!("quill-text2sql-{name}-{}", &suffix[..12]));
    std::fs::create_dir_all(&dir).expect("create temp directory");
    let path = dir.join("shop.sqlite").to_string_lossy().into_owned();

    let pool = dbx_core::db::sqlite::connect_path_create_if_missing(&path).await.expect("open sqlite");
    let mut ddl = SHOP_DDL.to_string();
    for i in 0..filler_tables {
        ddl.push_str(&format!(" CREATE TABLE metric_snapshot_{i} (id INTEGER PRIMARY KEY, payload TEXT, level TEXT);"));
    }
    dbx_core::db::sqlite::execute_query(&pool, &ddl).await.expect("seed schema");

    let storage = dbx_core::persistence::test_storage::open(&dir.join("storage.db")).await.expect("open storage");
    let state = Arc::new(AppState::new(storage));
    let connection_id = format!("text2sql-{name}-{suffix}");
    state.configs.write().await.insert(connection_id.clone(), sqlite_config(&connection_id, &path));
    (state, connection_id)
}

async fn search(state: &Arc<AppState>, connection_id: &str, question: &str) -> String {
    let call = ToolCall {
        id: "call-1".into(),
        name: "search_schema".into(),
        arguments: json!({ "question": question }),
        provider_payload: None,
    };
    let result = execute_tool(
        &call,
        state,
        connection_id,
        "main",
        Some("main"),
        &DatabaseType::Sqlite,
        AgentSqlPermissions::default(),
    )
    .await;
    assert!(!result.is_error, "search_schema failed: {}", result.content);
    result.content
}

#[tokio::test]
async fn small_schema_is_returned_whole_with_foreign_keys() {
    let (state, connection_id) = shop_state("small", 0).await;
    let output = search(&state, &connection_id, "top customers by revenue").await;

    assert!(output.starts_with("Full schema (4 tables):"), "{output}");
    assert!(output.contains("orders (BASE TABLE)"), "{output}");
    assert!(output.contains("customer_id INTEGER NOT NULL -> customers.id"), "{output}");
    assert!(output.contains("product_id INTEGER NOT NULL -> products.id"), "{output}");
}

#[tokio::test]
async fn large_schema_links_the_question_and_its_join_path() {
    // 200 unrelated tables push the schema past both the full-detail loading
    // threshold and the rendering budget, so ranking has to find the shop.
    let (state, connection_id) = shop_state("large", 200).await;
    let output = search(&state, &connection_id, "which customers bought which products").await;

    assert!(output.starts_with("Tables relevant to \"which customers bought which products\""), "{output}");
    for table in ["customers (BASE TABLE)", "products (BASE TABLE)", "orders (BASE TABLE)", "order_items (BASE TABLE)"]
    {
        assert!(output.contains(table), "missing {table} in:\n{output}");
    }
    assert!(!output.contains("metric_snapshot_0 (BASE TABLE)"), "unrelated tables must not be expanded:\n{output}");
    assert!(output.contains("Other tables (200), not shown:"), "{output}");
}

#[tokio::test]
async fn unmatched_question_lists_table_names_instead_of_guessing() {
    let (state, connection_id) = shop_state("unmatched", 200).await;
    let output = search(&state, &connection_id, "weather forecast").await;

    assert!(output.starts_with("No table matched \"weather forecast\""), "{output}");
    assert!(output.contains("customers"), "{output}");
}

#[test]
fn search_schema_is_offered_only_for_relational_engines() {
    use dbx_core::ai::agent_tools::{all_tools, read_only_tools};
    let has = |tools: Vec<dbx_core::agent_events::ToolDefinition>| tools.iter().any(|t| t.name == "search_schema");

    assert!(has(read_only_tools(DatabaseType::Postgres)));
    assert!(has(all_tools(DatabaseType::Sqlite, AgentSqlPermissions::default())));
    assert!(!has(all_tools(DatabaseType::MongoDb, AgentSqlPermissions::default())));
    assert!(!has(all_tools(DatabaseType::Redis, AgentSqlPermissions::default())));
    assert!(!has(read_only_tools(DatabaseType::Qdrant)));
}
