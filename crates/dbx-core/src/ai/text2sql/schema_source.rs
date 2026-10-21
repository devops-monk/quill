//! Load table documents for schema linking from a live connection.
//!
//! Column and foreign-key metadata costs one or two metadata queries per
//! table, so loading is bounded:
//! - Schemas up to `FULL_DETAIL_TABLES` tables load every table's details once.
//! - Larger schemas rank tables by name and comment first, then load details
//!   only for the best `CANDIDATE_TABLES` candidates and the tables their
//!   foreign keys point at. Details accumulate in the cache, so repeated
//!   questions about the same area of a big schema get cheaper.
//! - All detail loading stops at `DETAIL_LOAD_BUDGET`; tables not reached
//!   still appear by name.
//!
//! The cache lives for `CACHE_TTL` and is dropped by [`invalidate`] when the
//! user refreshes metadata.

use std::collections::HashMap;
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use super::linker::{ColumnDoc, ForeignKeyDoc, SchemaIndex, TableDoc};
use crate::connection::AppState;
use crate::types::TableInfo;

const FULL_DETAIL_TABLES: usize = 150;
const CANDIDATE_TABLES: usize = 24;
const DETAIL_LOAD_BUDGET: Duration = Duration::from_secs(20);
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Default)]
struct TableDetails {
    columns: Vec<ColumnDoc>,
    foreign_keys: Vec<ForeignKeyDoc>,
}

struct CachedSchema {
    loaded_at: Instant,
    tables: Vec<TableInfo>,
    details: HashMap<String, TableDetails>,
}

type CacheKey = (String, String, String);

fn cache() -> &'static StdMutex<HashMap<CacheKey, CachedSchema>> {
    static CACHE: OnceLock<StdMutex<HashMap<CacheKey, CachedSchema>>> = OnceLock::new();
    CACHE.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn lock_cache() -> std::sync::MutexGuard<'static, HashMap<CacheKey, CachedSchema>> {
    // A panic while holding the lock leaves only a partially warmed cache
    // behind, which is safe to keep using.
    cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Drop cached schema documents for a connection, e.g. after a metadata refresh.
pub fn invalidate(connection_id: &str) {
    lock_cache().retain(|(cached_connection, _, _), _| cached_connection != connection_id);
}

/// Table documents for one question, plus how complete they are.
pub struct LoadedSchema {
    pub docs: Vec<TableDoc>,
    pub total_tables: usize,
    /// True when some tables appear by name only.
    pub partial: bool,
}

pub async fn load_for_question(
    state: &AppState,
    connection_id: &str,
    database: &str,
    schema: &str,
    question: &str,
) -> Result<LoadedSchema, String> {
    let key: CacheKey = (connection_id.to_string(), database.to_string(), schema.to_string());

    let cached = {
        let mut cache = lock_cache();
        cache.retain(|_, entry| entry.loaded_at.elapsed() < CACHE_TTL);
        cache.get(&key).map(|entry| (entry.tables.clone(), entry.details.clone()))
    };
    let (tables, mut details) = match cached {
        Some(found) => found,
        None => {
            let tables =
                crate::schema::list_tables_core(state, connection_id, database, schema, None, None, None, None, None)
                    .await
                    .map_err(|e| format!("Failed to list tables: {e}"))?;
            (tables, HashMap::new())
        }
    };

    let targets: Vec<String> = if tables.len() <= FULL_DETAIL_TABLES {
        tables.iter().map(|table| table.name.clone()).collect()
    } else {
        let name_index = SchemaIndex::new(build_docs(&tables, &details));
        name_index.search(question, CANDIDATE_TABLES).into_iter().map(|hit| tables[hit.index].name.clone()).collect()
    };

    let deadline = Instant::now() + DETAIL_LOAD_BUDGET;
    let mut queue: Vec<String> = targets;
    let mut position = 0;
    let follow_foreign_keys = tables.len() > FULL_DETAIL_TABLES;
    while position < queue.len() && Instant::now() < deadline {
        let name = queue[position].clone();
        position += 1;
        if !details.contains_key(&name) {
            let loaded = load_details(state, connection_id, database, schema, &name).await;
            details.insert(name.clone(), loaded);
        }
        // In large-schema mode, load the tables a candidate joins to as well,
        // so FK expansion has columns to show.
        if follow_foreign_keys && position <= CANDIDATE_TABLES {
            for fk in &details[&name].foreign_keys {
                if !queue.contains(&fk.ref_table) && tables.iter().any(|t| t.name == fk.ref_table) {
                    queue.push(fk.ref_table.clone());
                }
            }
        }
    }

    {
        let mut cache = lock_cache();
        let entry = cache.entry(key).or_insert_with(|| CachedSchema {
            loaded_at: Instant::now(),
            tables: tables.clone(),
            details: HashMap::new(),
        });
        entry.details.extend(details.iter().map(|(name, detail)| (name.clone(), detail.clone())));
    }

    let docs = build_docs(&tables, &details);
    let partial = docs.iter().any(|doc| !doc.columns_loaded);
    Ok(LoadedSchema { total_tables: tables.len(), docs, partial })
}

fn build_docs(tables: &[TableInfo], details: &HashMap<String, TableDetails>) -> Vec<TableDoc> {
    tables
        .iter()
        .map(|table| {
            let detail = details.get(&table.name);
            TableDoc {
                name: table.name.clone(),
                kind: table.table_type.clone(),
                comment: table.comment.clone().filter(|comment| !comment.trim().is_empty()),
                columns: detail.map(|d| d.columns.clone()).unwrap_or_default(),
                foreign_keys: detail.map(|d| d.foreign_keys.clone()).unwrap_or_default(),
                columns_loaded: detail.is_some_and(|d| !d.columns.is_empty()),
            }
        })
        .collect()
}

/// Columns and foreign keys for one table. Failures degrade to "name only"
/// rather than failing the whole lookup: one unreadable view must not hide
/// the rest of the schema.
async fn load_details(state: &AppState, connection_id: &str, database: &str, schema: &str, table: &str) -> TableDetails {
    let columns = match crate::schema::get_columns_core(state, connection_id, database, schema, table).await {
        Ok(columns) => columns
            .into_iter()
            .map(|column| ColumnDoc {
                name: column.name,
                data_type: column.data_type,
                primary_key: column.is_primary_key,
                nullable: column.is_nullable,
                comment: column.comment.filter(|comment| !comment.trim().is_empty()),
            })
            .collect(),
        Err(error) => {
            log::debug!("[text2sql] columns unavailable for {table}: {error}");
            Vec::new()
        }
    };
    let foreign_keys = match crate::schema::list_foreign_keys_core(state, connection_id, database, schema, table).await {
        Ok(keys) => keys
            .into_iter()
            .map(|key| ForeignKeyDoc { column: key.column, ref_table: key.ref_table, ref_column: key.ref_column })
            .collect(),
        Err(error) => {
            log::debug!("[text2sql] foreign keys unavailable for {table}: {error}");
            Vec::new()
        }
    };
    TableDetails { columns, foreign_keys }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str) -> TableInfo {
        TableInfo {
            name: name.into(),
            table_type: "TABLE".into(),
            valid: None,
            comment: Some("  ".into()),
            parent_schema: None,
            parent_name: None,
        }
    }

    #[test]
    fn build_docs_marks_tables_without_details_as_name_only() {
        let mut details = HashMap::new();
        details.insert(
            "orders".to_string(),
            TableDetails {
                columns: vec![ColumnDoc { name: "id".into(), data_type: "int".into(), ..Default::default() }],
                foreign_keys: vec![],
            },
        );
        let docs = build_docs(&[info("orders"), info("customers")], &details);
        assert!(docs[0].columns_loaded);
        assert!(!docs[1].columns_loaded);
        assert_eq!(docs[0].comment, None, "blank comments are dropped");
    }

    #[test]
    fn invalidate_drops_only_the_named_connection() {
        let entry = || CachedSchema { loaded_at: Instant::now(), tables: vec![], details: HashMap::new() };
        {
            let mut cache = lock_cache();
            cache.insert(("text2sql-a".into(), "db".into(), "".into()), entry());
            cache.insert(("text2sql-b".into(), "db".into(), "".into()), entry());
        }
        invalidate("text2sql-a");
        let cache = lock_cache();
        assert!(!cache.contains_key(&("text2sql-a".into(), "db".into(), "".into())));
        assert!(cache.contains_key(&("text2sql-b".into(), "db".into(), "".into())));
    }
}
