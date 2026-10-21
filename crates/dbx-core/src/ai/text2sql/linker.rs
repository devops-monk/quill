//! Schema linking: pick the tables a natural-language question is about.
//!
//! Adapted from DB-GPT's schema retriever (`dbgpt_ext/rag/summary/rdbms_db_summary.py`
//! and `dbgpt_ext/rag/retriever/db_schema.py`): every table becomes one
//! document, the question retrieves the closest documents, and the model only
//! sees those. DB-GPT ranks with an embedding model; this module ranks with
//! BM25 over identifier-aware tokens instead, so linking needs no model
//! download and no network call, and schema text never leaves the machine.
//!
//! Two additions over plain retrieval:
//! - FK expansion: tables a hit references, and junction tables that connect
//!   two hits, are added so the model sees the join path it will need.
//! - Small schemas skip retrieval and are returned whole, because ranking can
//!   only lose information when everything already fits the budget.

use std::collections::{HashMap, HashSet};

/// One column as the linker sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColumnDoc {
    pub name: String,
    pub data_type: String,
    pub primary_key: bool,
    pub nullable: bool,
    pub comment: Option<String>,
}

/// One outgoing foreign key of a table.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKeyDoc {
    pub column: String,
    pub ref_table: String,
    pub ref_column: String,
}

/// Everything the linker knows about one table. `columns_loaded` is false when
/// only the table list was fetched; such a table can still be ranked by its
/// name and comment, and renders as a one-line entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableDoc {
    pub name: String,
    pub kind: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnDoc>,
    pub foreign_keys: Vec<ForeignKeyDoc>,
    pub columns_loaded: bool,
}

/// Columns rendered per table before the rest are summarized as a count.
const MAX_RENDERED_COLUMNS: usize = 60;

/// Comments are single-lined and capped so one verbose comment cannot crowd
/// other tables out of the budget.
const MAX_RENDERED_COMMENT_CHARS: usize = 160;

/// Weight of a query term that came from synonym expansion rather than the
/// user's own words.
const SYNONYM_WEIGHT: f32 = 0.4;

/// Field weights: the table name is the strongest signal, column names next.
const WEIGHT_TABLE_NAME: f32 = 3.0;
const WEIGHT_TABLE_COMMENT: f32 = 1.0;
const WEIGHT_COLUMN_NAME: f32 = 1.0;
const WEIGHT_COLUMN_COMMENT: f32 = 0.5;
const WEIGHT_FK_TARGET: f32 = 0.5;

/// Longest chain of intermediate tables searched when connecting two hits.
const MAX_JOIN_PATH_TABLES: usize = 2;

const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;

const STOPWORDS: &[&str] = &[
    "a", "about", "all", "an", "and", "any", "are", "as", "at", "be", "by", "can", "could", "did", "do", "does",
    "each", "for", "from", "get", "give", "have", "how", "i", "in", "into", "is", "it", "its", "list", "me", "much",
    "many", "my", "of", "on", "or", "our", "please", "show", "tell", "than", "that", "the", "their", "them", "there",
    "these", "this", "those", "to", "was", "we", "were", "what", "when", "where", "which", "who", "whose", "why",
    "with", "would", "you", "your",
];

/// Small business-vocabulary bridge for words users say that schemas rarely
/// spell the same way. Expansions score at `SYNONYM_WEIGHT`, so a literal
/// match always wins over a synonym match.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("revenue", &["amount", "total", "price", "sale", "payment", "invoice"]),
    ("sale", &["order", "invoice", "revenue"]),
    ("customer", &["client", "user", "account", "buyer"]),
    ("client", &["customer", "account"]),
    ("user", &["customer", "account", "member"]),
    ("product", &["item", "sku", "good"]),
    ("item", &["product", "line"]),
    ("purchase", &["order", "sale"]),
    ("employee", &["staff", "worker"]),
    ("staff", &["employee"]),
    ("salary", &["pay", "wage", "compensation"]),
    ("price", &["amount", "cost"]),
    ("cost", &["price", "amount"]),
    ("spend", &["amount", "cost", "payment"]),
    ("signup", &["registration", "created"]),
    ("country", &["nation", "region"]),
    ("city", &["town", "location"]),
];

/// Split text into normalized search tokens.
///
/// `customerOrders`, `customer_orders` and `Customer Orders` all produce
/// `["customer", "order"]`. CJK characters become single-character tokens so a
/// Chinese question can still match Chinese comments.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        let mut ascii_run = String::new();
        for ch in word.chars() {
            if is_cjk(ch) {
                push_word_parts(&ascii_run, &mut tokens);
                ascii_run.clear();
                tokens.push(ch.to_string());
            } else {
                ascii_run.push(ch);
            }
        }
        push_word_parts(&ascii_run, &mut tokens);
    }
    tokens
}

fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}')
}

fn push_word_parts(word: &str, tokens: &mut Vec<String>) {
    for part in split_camel_case(word) {
        let lower = part.to_lowercase();
        if lower.is_empty() || STOPWORDS.contains(&lower.as_str()) {
            continue;
        }
        tokens.push(stem(&lower));
    }
}

/// `HTTPServerID2` -> `["HTTP", "Server", "ID", "2"]`.
fn split_camel_case(word: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let (idx, cur) = chars[i];
        let prev = chars[i - 1].1;
        let next = chars.get(i + 1).map(|(_, c)| *c);
        let lower_to_upper = prev.is_lowercase() && cur.is_uppercase();
        let acronym_end = prev.is_uppercase() && cur.is_uppercase() && next.is_some_and(|n| n.is_lowercase());
        let digit_edge = prev.is_ascii_digit() != cur.is_ascii_digit();
        if lower_to_upper || acronym_end || digit_edge {
            parts.push(&word[start..idx]);
            start = idx;
        }
    }
    if start < word.len() {
        parts.push(&word[start..]);
    }
    parts
}

/// Light plural folding: enough for `orders`/`order` and `categories`/`category`
/// to meet, without a stemming library.
fn stem(word: &str) -> String {
    let len = word.len();
    if !word.is_ascii() || len <= 3 {
        return word.to_string();
    }
    if let Some(base) = word.strip_suffix("ies") {
        if len > 4 {
            return format!("{base}y");
        }
    }
    for suffix in ["sses", "shes", "ches", "xes"] {
        if word.ends_with(suffix) {
            return word[..len - 2].to_string();
        }
    }
    if word.ends_with('s') && !word.ends_with("ss") && !word.ends_with("us") && !word.ends_with("is") {
        return word[..len - 1].to_string();
    }
    word.to_string()
}

/// Query terms with weights: the user's own words at 1.0, synonyms lower.
fn query_terms(question: &str) -> HashMap<String, f32> {
    let mut terms: HashMap<String, f32> = HashMap::new();
    for token in tokenize(question) {
        terms.insert(token, 1.0);
    }
    let literal: Vec<String> = terms.keys().cloned().collect();
    for token in literal {
        if let Some((_, expansions)) = SYNONYMS.iter().find(|(word, _)| *word == token) {
            for expansion in *expansions {
                let entry = terms.entry(stem(expansion)).or_insert(0.0);
                *entry = entry.max(SYNONYM_WEIGHT);
            }
        }
    }
    terms
}

/// A scored retrieval hit.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredTable {
    pub index: usize,
    pub score: f32,
}

/// The tables chosen for one question, in the order they should be shown.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaLink {
    /// Indexes into the docs passed to [`SchemaIndex::new`].
    pub tables: Vec<usize>,
    /// Subset of `tables` added for joins rather than matched by the question.
    pub join_tables: HashSet<usize>,
    /// True when the whole schema fit the budget and retrieval was skipped.
    pub full_schema: bool,
}

/// Options for [`SchemaIndex::link`].
#[derive(Debug, Clone, Copy)]
pub struct LinkOptions {
    /// Maximum tables matched by retrieval, before FK expansion.
    pub max_tables: usize,
    /// Extra tables FK expansion may add on top of `max_tables`.
    pub max_join_tables: usize,
    /// Rendered-schema character budget. The whole schema is returned when it
    /// fits; otherwise linked tables are added until the budget is spent.
    pub char_budget: usize,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self { max_tables: 8, max_join_tables: 4, char_budget: 12_000 }
    }
}

/// BM25 index over table documents.
pub struct SchemaIndex {
    docs: Vec<TableDoc>,
    term_weights: Vec<HashMap<String, f32>>,
    doc_lengths: Vec<f32>,
    avg_doc_length: f32,
    doc_freq: HashMap<String, usize>,
    name_tokens: Vec<HashSet<String>>,
    by_name: HashMap<String, usize>,
    /// Undirected FK adjacency, used to find join paths between hits.
    neighbors: Vec<Vec<usize>>,
}

impl SchemaIndex {
    pub fn new(docs: Vec<TableDoc>) -> Self {
        let mut term_weights = Vec::with_capacity(docs.len());
        let mut doc_lengths = Vec::with_capacity(docs.len());
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        let mut name_tokens = Vec::with_capacity(docs.len());
        let mut by_name = HashMap::new();

        for (index, doc) in docs.iter().enumerate() {
            let mut weights: HashMap<String, f32> = HashMap::new();
            let mut add = |text: &str, weight: f32| {
                for token in tokenize(text) {
                    *weights.entry(token).or_insert(0.0) += weight;
                }
            };
            add(&doc.name, WEIGHT_TABLE_NAME);
            if let Some(comment) = &doc.comment {
                add(comment, WEIGHT_TABLE_COMMENT);
            }
            for column in &doc.columns {
                add(&column.name, WEIGHT_COLUMN_NAME);
                if let Some(comment) = &column.comment {
                    add(comment, WEIGHT_COLUMN_COMMENT);
                }
            }
            for fk in &doc.foreign_keys {
                add(&fk.ref_table, WEIGHT_FK_TARGET);
            }
            for term in weights.keys() {
                *doc_freq.entry(term.clone()).or_insert(0) += 1;
            }
            doc_lengths.push(weights.values().sum::<f32>());
            term_weights.push(weights);
            name_tokens.push(tokenize(&doc.name).into_iter().collect());
            by_name.entry(doc.name.to_lowercase()).or_insert(index);
        }

        let avg_doc_length =
            if doc_lengths.is_empty() { 1.0 } else { doc_lengths.iter().sum::<f32>() / doc_lengths.len() as f32 };
        let mut index = Self {
            docs,
            term_weights,
            doc_lengths,
            avg_doc_length: avg_doc_length.max(1.0),
            doc_freq,
            name_tokens,
            by_name,
            neighbors: Vec::new(),
        };
        index.neighbors = vec![Vec::new(); index.docs.len()];
        for (source, doc) in index.docs.iter().enumerate() {
            for fk in &doc.foreign_keys {
                if let Some(target) = index.find(&fk.ref_table).filter(|&target| target != source) {
                    index.neighbors[source].push(target);
                    index.neighbors[target].push(source);
                }
            }
        }
        for list in &mut index.neighbors {
            list.sort_unstable();
            list.dedup();
        }
        index
    }

    pub fn docs(&self) -> &[TableDoc] {
        &self.docs
    }

    /// Resolve a table name as written in a foreign key (case-insensitive,
    /// with or without a schema prefix).
    pub fn find(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        self.by_name.get(&lower).copied().or_else(|| {
            let unqualified = lower.rsplit('.').next().unwrap_or(&lower).trim_matches(|c| c == '"' || c == '`');
            self.by_name.get(unqualified).copied()
        })
    }

    /// Intermediate tables on the shortest FK path from `from` to `to`, walking
    /// foreign keys in either direction. Empty when the tables are adjacent or
    /// no path of at most `max_middle` intermediate tables exists.
    fn join_path(&self, from: usize, to: usize, max_middle: usize) -> Vec<usize> {
        let mut previous: HashMap<usize, usize> = HashMap::new();
        let mut frontier = vec![from];
        let mut seen: HashSet<usize> = HashSet::from([from]);
        for _ in 0..=max_middle {
            let mut next = Vec::new();
            for &node in &frontier {
                for &neighbor in &self.neighbors[node] {
                    if !seen.insert(neighbor) {
                        continue;
                    }
                    previous.insert(neighbor, node);
                    if neighbor == to {
                        let mut path = Vec::new();
                        let mut cursor = previous[&to];
                        while cursor != from {
                            path.push(cursor);
                            cursor = previous[&cursor];
                        }
                        path.reverse();
                        return path;
                    }
                    next.push(neighbor);
                }
            }
            frontier = next;
        }
        Vec::new()
    }

    /// Rank tables against `question`; returns at most `k` hits with a positive score.
    pub fn search(&self, question: &str, k: usize) -> Vec<ScoredTable> {
        let terms = query_terms(question);
        if terms.is_empty() || k == 0 {
            return Vec::new();
        }
        let literal: HashSet<&String> = terms.iter().filter(|(_, w)| **w >= 1.0).map(|(t, _)| t).collect();
        let total_docs = self.docs.len() as f32;

        let mut hits: Vec<ScoredTable> = self
            .term_weights
            .iter()
            .enumerate()
            .filter_map(|(index, weights)| {
                let norm = 1.0 - BM25_B + BM25_B * self.doc_lengths[index] / self.avg_doc_length;
                let mut score = 0.0;
                for (term, query_weight) in &terms {
                    let Some(tf) = weights.get(term) else { continue };
                    let df = *self.doc_freq.get(term).unwrap_or(&0) as f32;
                    let idf = (1.0 + (total_docs - df + 0.5) / (df + 0.5)).ln();
                    score += query_weight * idf * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * norm);
                }
                // The question naming the whole table ("order items") is the
                // strongest evidence there is; reward it beyond per-term BM25.
                let name = &self.name_tokens[index];
                if score > 0.0 && !name.is_empty() && name.iter().all(|token| literal.contains(token)) {
                    score *= 1.5;
                }
                (score > 0.0).then_some(ScoredTable { index, score })
            })
            .collect();

        hits.sort_by(|a, b| {
            b.score.total_cmp(&a.score).then_with(|| self.docs[a.index].name.cmp(&self.docs[b.index].name))
        });
        hits.truncate(k);
        hits
    }

    /// Choose the tables to show the model for `question`.
    pub fn link(&self, question: &str, options: LinkOptions) -> SchemaLink {
        let full_render: usize = self.docs.iter().map(|doc| render_table(doc).len() + 1).sum();
        if full_render <= options.char_budget {
            return SchemaLink {
                tables: (0..self.docs.len()).collect(),
                join_tables: HashSet::new(),
                full_schema: true,
            };
        }

        let hits: Vec<usize> = self.search(question, options.max_tables).into_iter().map(|hit| hit.index).collect();
        let hit_set: HashSet<usize> = hits.iter().copied().collect();
        let mut tables = hits.clone();
        let mut join_tables = HashSet::new();
        let add_join = |index: usize, tables: &mut Vec<usize>, join_tables: &mut HashSet<usize>| {
            if join_tables.len() < options.max_join_tables && !hit_set.contains(&index) && join_tables.insert(index) {
                tables.push(index);
            }
        };

        // Tables on the shortest FK path between two hits come first: without
        // them the model cannot join the hits at all
        // (customers <- orders <- order_items -> products).
        for (position, &from) in hits.iter().enumerate() {
            for &to in &hits[position + 1..] {
                for middle in self.join_path(from, to, MAX_JOIN_PATH_TABLES) {
                    add_join(middle, &mut tables, &mut join_tables);
                }
            }
        }
        // Then dimension tables the hits point at (orders -> customers).
        for &hit in &hits {
            for fk in &self.docs[hit].foreign_keys {
                if let Some(target) = self.find(&fk.ref_table) {
                    add_join(target, &mut tables, &mut join_tables);
                }
            }
        }

        // Spend the budget in rank order; always keep the best match.
        let mut used = 0;
        let mut kept = Vec::with_capacity(tables.len());
        for index in tables {
            let size = render_table(&self.docs[index]).len() + 1;
            if !kept.is_empty() && used + size > options.char_budget {
                continue;
            }
            used += size;
            kept.push(index);
        }
        join_tables.retain(|index| kept.contains(index));
        SchemaLink { tables: kept, join_tables, full_schema: false }
    }

    /// Render the linked tables, plus a compact list of other table names when
    /// retrieval was used so the model knows what else exists.
    pub fn render_link(&self, link: &SchemaLink, char_budget: usize) -> String {
        let mut out = String::new();
        for &index in &link.tables {
            out.push_str(&render_table(&self.docs[index]));
            if link.join_tables.contains(&index) {
                out.push_str("  (included for joins)\n");
            }
            out.push('\n');
        }
        if link.full_schema {
            return out;
        }

        let shown: HashSet<usize> = link.tables.iter().copied().collect();
        let others: Vec<&str> =
            self.docs.iter().enumerate().filter(|(i, _)| !shown.contains(i)).map(|(_, d)| d.name.as_str()).collect();
        if others.is_empty() {
            return out;
        }
        out.push_str(&format!("Other tables ({}), not shown:", others.len()));
        let mut listed = 0;
        for name in &others {
            if out.len() + name.len() + 2 > char_budget {
                break;
            }
            out.push_str(if listed == 0 { " " } else { ", " });
            out.push_str(name);
            listed += 1;
        }
        if listed < others.len() {
            out.push_str(&format!(", ... ({} more)", others.len() - listed));
        }
        out.push('\n');
        out
    }
}

fn single_line(text: &str, max_chars: usize) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= max_chars {
        return Some(collapsed);
    }
    Some(format!("{}…", collapsed.chars().take(max_chars).collect::<String>()))
}

/// Render one table in a compact DDL-like form:
///
/// ```text
/// orders (TABLE) -- Customer orders
///   id bigint PK
///   customer_id bigint NOT NULL -> customers.id
/// ```
pub fn render_table(doc: &TableDoc) -> String {
    let mut out = format!("{} ({})", doc.name, if doc.kind.is_empty() { "TABLE" } else { &doc.kind });
    if let Some(comment) = doc.comment.as_deref().and_then(|c| single_line(c, MAX_RENDERED_COMMENT_CHARS)) {
        out.push_str(&format!(" -- {comment}"));
    }
    out.push('\n');
    if !doc.columns_loaded {
        out.push_str("  (columns not loaded; call get_columns)\n");
        return out;
    }

    let fk_by_column: HashMap<&str, &ForeignKeyDoc> =
        doc.foreign_keys.iter().map(|fk| (fk.column.as_str(), fk)).collect();
    for column in doc.columns.iter().take(MAX_RENDERED_COLUMNS) {
        out.push_str(&format!("  {} {}", column.name, column.data_type));
        if column.primary_key {
            out.push_str(" PK");
        } else if !column.nullable {
            out.push_str(" NOT NULL");
        }
        if let Some(fk) = fk_by_column.get(column.name.as_str()) {
            out.push_str(&format!(" -> {}.{}", fk.ref_table, fk.ref_column));
        }
        if let Some(comment) = column.comment.as_deref().and_then(|c| single_line(c, MAX_RENDERED_COMMENT_CHARS)) {
            out.push_str(&format!(" -- {comment}"));
        }
        out.push('\n');
    }
    if doc.columns.len() > MAX_RENDERED_COLUMNS {
        out.push_str(&format!(
            "  ... ({} more columns; call get_columns for the full list)\n",
            doc.columns.len() - MAX_RENDERED_COLUMNS
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, data_type: &str) -> ColumnDoc {
        ColumnDoc { name: name.into(), data_type: data_type.into(), nullable: true, ..Default::default() }
    }

    fn pk(name: &str) -> ColumnDoc {
        ColumnDoc { name: name.into(), data_type: "bigint".into(), primary_key: true, ..Default::default() }
    }

    fn fk(column: &str, ref_table: &str) -> ForeignKeyDoc {
        ForeignKeyDoc { column: column.into(), ref_table: ref_table.into(), ref_column: "id".into() }
    }

    fn table(name: &str, columns: Vec<ColumnDoc>, foreign_keys: Vec<ForeignKeyDoc>) -> TableDoc {
        TableDoc { name: name.into(), kind: "TABLE".into(), columns, foreign_keys, columns_loaded: true, comment: None }
    }

    /// A shop schema plus enough unrelated tables that the full schema
    /// overflows a small budget and retrieval has to do the work.
    fn shop() -> Vec<TableDoc> {
        let mut docs = vec![
            table("customers", vec![pk("id"), col("name", "text"), col("country", "text")], vec![]),
            table("products", vec![pk("id"), col("title", "text"), col("unit_price", "numeric")], vec![]),
            table(
                "orders",
                vec![
                    pk("id"),
                    col("customer_id", "bigint"),
                    col("total_amount", "numeric"),
                    col("created_at", "timestamp"),
                ],
                vec![fk("customer_id", "customers")],
            ),
            table(
                "order_items",
                vec![pk("id"), col("order_id", "bigint"), col("product_id", "bigint"), col("quantity", "int")],
                vec![fk("order_id", "orders"), fk("product_id", "products")],
            ),
            table("employees", vec![pk("id"), col("full_name", "text"), col("salary", "numeric")], vec![]),
        ];
        for i in 0..40 {
            docs.push(table(
                &format!("audit_log_{i}"),
                vec![pk("id"), col("payload", "jsonb"), col("level", "text")],
                vec![],
            ));
        }
        docs
    }

    fn names(index: &SchemaIndex, link: &SchemaLink) -> Vec<String> {
        link.tables.iter().map(|&i| index.docs()[i].name.clone()).collect()
    }

    #[test]
    fn tokenize_splits_identifiers_and_folds_plurals() {
        assert_eq!(tokenize("customerOrders"), vec!["customer", "order"]);
        assert_eq!(tokenize("order_items"), vec!["order", "item"]);
        assert_eq!(tokenize("HTTPServerID"), vec!["http", "server", "id"]);
        assert_eq!(tokenize("Show me the categories"), vec!["category"]);
        assert_eq!(tokenize("addresses status"), vec!["address", "status"]);
    }

    #[test]
    fn tokenize_keeps_cjk_characters() {
        assert_eq!(tokenize("订单"), vec!["订", "单"]);
    }

    #[test]
    fn search_ranks_the_named_table_first() {
        let index = SchemaIndex::new(shop());
        let hits = index.search("how many orders were placed last month", 3);
        assert_eq!(index.docs()[hits[0].index].name, "orders");
    }

    #[test]
    fn search_matches_columns_and_synonyms() {
        let index = SchemaIndex::new(shop());
        let hits = index.search("average salary", 1);
        assert_eq!(index.docs()[hits[0].index].name, "employees");
        // "revenue" appears nowhere; the synonym bridge reaches total_amount.
        let hits = index.search("revenue", 1);
        assert_eq!(index.docs()[hits[0].index].name, "orders");
    }

    #[test]
    fn search_returns_nothing_for_unrelated_questions() {
        let index = SchemaIndex::new(shop());
        assert!(index.search("what is the weather", 5).is_empty());
    }

    #[test]
    fn small_schema_is_returned_whole() {
        let index = SchemaIndex::new(shop().into_iter().take(5).collect());
        let link = index.link("anything", LinkOptions::default());
        assert!(link.full_schema);
        assert_eq!(link.tables.len(), 5);
    }

    #[test]
    fn link_adds_referenced_tables_for_joins() {
        let index = SchemaIndex::new(shop());
        let options = LinkOptions { max_tables: 1, max_join_tables: 4, char_budget: 1_500 };
        let link = index.link("total order amount", options);
        assert!(!link.full_schema);
        let linked = names(&index, &link);
        assert_eq!(linked[0], "orders");
        assert!(linked.contains(&"customers".to_string()), "{linked:?}");
        let customers = index.find("customers").unwrap();
        assert!(link.join_tables.contains(&customers));
    }

    #[test]
    fn link_adds_the_join_path_between_hits() {
        let index = SchemaIndex::new(shop());
        let options = LinkOptions { max_tables: 2, max_join_tables: 4, char_budget: 2_000 };
        let link = index.link("which customers bought which products", options);
        let linked = names(&index, &link);
        // customers <- orders <- order_items -> products: both middle tables
        // are needed and neither is named in the question.
        assert_eq!(
            linked[..2].iter().collect::<HashSet<_>>(),
            HashSet::from([&"customers".into(), &"products".into()])
        );
        assert!(linked.contains(&"orders".to_string()), "{linked:?}");
        assert!(linked.contains(&"order_items".to_string()), "{linked:?}");
        assert!(link.join_tables.contains(&index.find("order_items").unwrap()));
    }

    #[test]
    fn join_path_gives_up_beyond_the_depth_limit() {
        let chain: Vec<TableDoc> = (0..6)
            .map(|i| {
                let fks = if i == 0 { vec![] } else { vec![fk("prev_id", &format!("t{}", i - 1))] };
                table(&format!("t{i}"), vec![pk("id")], fks)
            })
            .collect();
        let index = SchemaIndex::new(chain);
        assert_eq!(index.join_path(0, 3, 2), vec![1, 2]);
        assert!(index.join_path(0, 5, 2).is_empty());
    }

    #[test]
    fn link_respects_the_character_budget_but_keeps_the_best_match() {
        let index = SchemaIndex::new(shop());
        let options = LinkOptions { max_tables: 8, max_join_tables: 4, char_budget: 10 };
        let link = index.link("orders", options);
        assert_eq!(names(&index, &link), vec!["orders"]);
    }

    #[test]
    fn find_resolves_schema_qualified_and_quoted_names() {
        let index = SchemaIndex::new(shop());
        assert_eq!(index.find("public.\"customers\""), index.find("customers"));
        assert_eq!(index.find("CUSTOMERS"), index.find("customers"));
    }

    #[test]
    fn render_marks_keys_foreign_keys_and_truncates_columns() {
        let mut doc = shop().remove(2);
        doc.comment = Some("Customer\norders".into());
        let rendered = render_table(&doc);
        assert!(rendered.starts_with("orders (TABLE) -- Customer orders\n"));
        assert!(rendered.contains("  id bigint PK\n"));
        assert!(rendered.contains("  customer_id bigint -> customers.id\n"));

        let wide = table("wide", (0..70).map(|i| col(&format!("c{i}"), "int")).collect(), vec![]);
        assert!(render_table(&wide).contains("... (10 more columns"));
    }

    #[test]
    fn render_link_lists_unshown_tables() {
        let index = SchemaIndex::new(shop());
        let options = LinkOptions { max_tables: 1, max_join_tables: 0, char_budget: 600 };
        let link = index.link("salary", options);
        let rendered = index.render_link(&link, 600);
        assert!(rendered.starts_with("employees (TABLE)"));
        assert!(rendered.contains("Other tables (44), not shown: customers"));
        assert!(rendered.contains("more)"));
    }
}
