//! Natural-language-to-SQL support for the agent ("Ask" in Quill).
//!
//! `linker` decides which tables a question is about; `schema_source` feeds it
//! from a live connection. The agent reaches both through the
//! `search_schema` tool in `agent_tools`.

pub mod linker;
pub mod schema_source;
