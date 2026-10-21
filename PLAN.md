# Quill: plan for an AI-native database client

> "Write your data in plain words."
> On approval, the first step copies this plan to `/Users/abhaypratapsingh/Documents/Personal/my-work/db/quill/PLAN.md`.

## Context
The goal is one desktop database client that works with every kind of database and that you can talk to. You pick a connection, ask a question in plain English, and Quill reads the schema, writes correct SQL for that dialect, runs it safely, and answers with a table and a chart. It combines the strengths of the two repos in `db/`:

| | **dbx** (base, Apache-2.0) | **DB-GPT** (AI ideas to port, MIT) |
|---|---|---|
| Stack | Tauri 2 + Rust workspace (`crates/*`) + Vue 3 / Tailwind 4 / reka-ui / CodeMirror 6 / ECharts | Python services + Next.js/React/antd |
| DBs | About 90 connection types (`plugins/connection-types/`), with native drivers plus JDBC and agent drivers | About 16 RDBMS connectors |
| AI today | An agent loop (`crates/dbx-core/src/ai/agent_loop.rs`) with tools `list_tables`, `get_columns`, `get_sample_data`, `execute_query` and `explain_query` (`agent_tools.rs`), write approval (`tool_approval.rs`), many LLM providers (`crates/dbx-ai-provider`), an intent router (`apps/desktop/src/lib/ai/aiIntentRouter.ts`), schema notes and a chart renderer | **Schema RAG** (`dbgpt_serve/datasource/service/db_summary_client.py`, `dbgpt_ext/rag/summary/rdbms_db_summary.py`), **LLM schema linking** (`dbgpt_ext/rag/schemalinker/schema_linking.py`), a strict Text2SQL prompt with display-type selection (`dbgpt_app/scene/chat_db/auto_execute/prompt.py`), **chart selection** (`openapi/api_v1/dashboard/chart_selection.py`, `chart_semantics.py`), dashboards and reports (`scene/chat_dashboard/`) |
| Gap | No retrieval over large schemas: the agent has to list everything. No chart auto-selection. No verified-query memory. | Heavy server stack, no desktop app, few drivers |

**Decision (confirmed):** fork dbx, rebrand it as **Quill**, and port DB-GPT's algorithms and prompts into Rust and Vue. DB-GPT's Python runtime is not shipped.

---

## Phase 0: Fork and rebrand
1. Copy `db/dbx` to `db/quill` with `rsync`, leaving out `node_modules`, `target` and `.git`. Then run `git init` in `db/quill` and make the first commit. The original repos stay untouched.
2. Rename the app:
   - In `src-tauri/tauri.conf.json` and the `tauri.*.conf.json` variants: `productName` becomes `Quill` and `identifier` becomes `app.quill.desktop`.
   - In `package.json`, set `name` to `quill`.
   - Update the window titles and the user-facing "DBX" strings in `apps/desktop/src/i18n/*`.
   - Rewrite `README.md`.
3. Remove upstream-specific endpoints: the updater URL in `tauri.conf.json` pointing at dbxio.com, the sponsor blocks and the cloud-sync defaults. Leave the auto-updater off until there's a Quill release channel.
4. Licensing: keep the Apache-2.0 `LICENSE`. Add a `NOTICE` file crediting dbx (Apache-2.0) and DB-GPT (MIT), and add DB-GPT's MIT text under `third_party/`.
5. Keep the internal crate names (`dbx-*`) for now, because renaming 27 crates gives no user value and creates large merge churn. Only names users can see change.

## Phase 1: New icon and visual identity
- **Concept:** a quill nib drawn on a rounded square. The nib's split slit is a glowing caret, and the nib's base is a three-layer database cylinder. The icon reads as "write" + "database" + "AI cursor".
- **Palette:** deep ink background `#151826`, nib gradient indigo `#6366F1` to violet `#A855F7`, caret accent amber `#F59E0B`.
- **Assets:** a master SVG at `assets/brand/quill-icon.svg`, plus monochrome and tray variants. Generate every platform size with `pnpm tauri icon assets/brand/quill-icon.svg`, which overwrites `src-tauri/icons/*`. Also update the favicon and the in-app logo component.
- **Design tokens:** rewrite `apps/desktop/src/styles/tokens.css` and `globals.css`:
  - Ink and indigo neutrals, with violet as the primary and amber as the AI accent.
  - Inter for the UI and JetBrains Mono for code and grids.
  - Radii of 8 and 12px, softer elevation, and AA contrast in both light and dark themes.
  - Keep the existing token names so all components restyle without being edited.

## Phase 2: The "Ask" engine (Text2SQL), in Rust
Add the module `crates/dbx-core/src/ai/text2sql/`, with SQL helpers going into `dbx-sql`.

1. **Schema indexer** (`text2sql/index.rs`)
   - Build one document per table, ported from `RdbmsSummary`'s format. Each document holds:
     - the table name and comment
     - columns with type, nullability and comment
     - PK/FK links
     - indexes
     - up to 3 distinct sample values per low-cardinality column
     - the user's schema notes (reusing the `aiSchemaNotes.ts` / `data/docs/annotations.rs` budgets and the untrusted-note markers)
   - Use the existing metadata APIs in `crates/dbx-core/src/schema/` and run the work in the background per connection and database.
   - Storage is a local SQLite table in `persistence/` holding the doc text, a hash and the embedding.
   - Re-index only when the table hash changes, triggered by the existing metadata invalidation in `connectionStore`.
   - Embeddings are computed on-device with `fastembed` (bge-small, ONNX), so schemas never leave the machine. If the embedding model can't load, fall back to BM25 keyword scoring.
2. **Schema linking** (`text2sql/linker.rs`)
   - If the full schema fits the token budget (about 6k tokens), send all of it and skip retrieval.
   - Otherwise:
     - run hybrid retrieval (vector + BM25) for the top-k tables (k=8);
     - expand one hop along FK edges so the needed join tables are included;
     - optionally have the LLM re-rank the candidates, porting DB-GPT's `_schema_linking_with_llm` prompt.
3. **Generation** (`text2sql/generate.rs` plus `prompt_template.rs`)
   - Port DB-GPT's constraint prompt:
     - use only the given tables;
     - add a `LIMIT {top_k}` unless the user asked for a count;
     - say "insufficient schema" instead of inventing tables;
     - follow the dialect named by `dbx-sql`'s dialect registry.
   - The output is strict JSON: `{thoughts, sql, display_type, assumptions[], tables_used[]}`.
   - Up to 3 **verified queries** are pulled in as few-shot examples (see item 6).
4. **Validate and repair loop** (`text2sql/validate.rs`)
   - Parse the SQL with `dbx-sql` for the target dialect.
   - Check that every table and column exists in the indexed schema.
   - Do a dry run with `EXPLAIN` through the existing `explain_query` path.
   - On a failure, send the error back to the model, up to 3 attempts.
   - Classify risk with the existing `dbx-sql` risk checks and `safety/`. **Read-only is the default.** Writes go through the existing `tool_approval.rs` confirmation card.
5. **Execute and explain**
   - Run the query with the existing row limit and timeout (`agent_query_timeout_secs`, `execute_query_row_limit`).
   - Stream a short plain-language answer.
   - Port the heuristics from `chart_selection.py` / `chart_semantics.py` to `apps/desktop/src/lib/ai/chartSelect.ts`, which maps column types and cardinality to bar, line, pie, scatter, table or KPI. Render the chart with the existing `components/ai/rich/AiChartRenderer.vue` (ECharts).
6. **Verified-query memory:** a "Save as verified" action stores the (question, SQL, connection) triple, which is embedded and later retrieved as few-shot examples. This lets Quill learn each team's own vocabulary.
7. **Agent integration**
   - Add a `search_schema(query)` tool to `agent_tools.rs` and change the system prompt in `agent_loop.rs` so the agent calls it first on large schemas. Existing tools and the MCP server (`crates/dbx-mcp`) get it automatically.
   - NoSQL engines (Mongo, Redis, Elasticsearch, etc.) keep using their current tools. The indexer covers collections and fields where the metadata exists.
8. **Expose it** as Tauri commands in `src-tauri/src/` (`ai_ask`, `ai_index_status`, `ai_reindex`), following the existing command and event streaming pattern in `agent_events.rs`.

## Phase 3: UI, making "Ask" the main surface
- **Ask bar:**
  - Open it with `⌘K`, or `⌘L` on a selected connection or database. It's a large prompt field with `@table` mentions (reusing `aiTableMentions.ts`) and suggested questions generated from the schema.
  - The answer panel is a split view with:
    - an answer card in plain words, with any assumptions;
    - chips showing the tables used, for transparency;
    - the SQL in an editable CodeMirror block;
    - a results grid and an auto chart, with tabs to switch between them;
    - actions: **Run again, Open in editor, Save as verified, Pin to dashboard, Export**.
- **Welcome and home screen:** a hero "Ask your data…" prompt, recent connections, and index status for each connection (indexed, indexing, or stale).
- **Sidebar and schema browser:** restyled with the new tokens, with an AI sparkle on hover ("Ask about this table").
- **Dashboards** (adapted from DB-GPT's `chat_dashboard`): a dashboard tab where pinned answers are laid out as tiles, each of which can be refreshed. Built on `splitpanes` and ECharts.
- **Settings → AI:**
  - Choose the provider, using the existing catalog in `useAiModelCatalog.ts`. The default is **Claude (claude-sonnet-5-5)**, with local **Ollama** for offline or private use.
  - An "allow writes" safety toggle for each connection.
  - An indexing toggle for each connection, plus a "never send sample values" privacy switch.
- Use the `frontend-design` skill when implementing the visual pass.

## Phase 4: Hardening and evaluation
- **Text2SQL eval harness** (`tests/text2sql-eval/`):
  - Use the Chinook and Northwind SQLite databases plus a Spider subset, with question and expected result set pairs.
  - Measure execution accuracy and the repair rate. Run it from a new script, `pnpm eval:text2sql`.
- Add prompt-injection guards: keep the untrusted-note markers, and treat schema comments and sample values as data, never as instructions.
- For large-schema performance, extend the existing `bench:sidebar-large-schema` with a retrieval benchmark on 5k tables.

---

## Critical files
**Modified:**
- `src-tauri/tauri.conf.json*`
- `src-tauri/icons/*`
- `apps/desktop/src/styles/tokens.css`, `globals.css`
- `crates/dbx-core/src/ai/agent_tools.rs`, `agent_loop.rs`, `prompt_template.rs`
- `apps/desktop/src/components/editor/AiAssistant.vue`
- `apps/desktop/src/i18n/*`
- `README.md`

**New:**
- `crates/dbx-core/src/ai/text2sql/{mod,index,linker,generate,validate,memory}.rs`
- `apps/desktop/src/components/ask/*` (AskBar, AnswerPanel, TablesUsedChips, DashboardView)
- `apps/desktop/src/lib/ai/chartSelect.ts`
- `assets/brand/quill-icon.svg`
- `NOTICE`
- `PLAN.md`

**Reused:**
- dbx: driver layer (`dbx-drivers`), dialects and risk checks (`dbx-sql`), `tool_approval.rs`, `AiChartRenderer.vue`, `aiTableMentions.ts`, `aiSchemaNotes.ts`, `dbx-ai-provider`
- Ported from DB-GPT: the prompts and algorithms in the files listed in Context

## Verification
1. `pnpm install && pnpm typecheck && pnpm test && pnpm test:architecture`
2. `cargo nextest run -p dbx-core --no-default-features --features sqlite-bundled`, plus new unit tests for the linker (FK expansion, budget cutoff), the validator (unknown column leads to a repair) and chart selection.
3. `pnpm eval:text2sql` on Chinook and Northwind, with a target of 80% or higher execution accuracy on Claude.
4. `pnpm dev:tauri`:
   - Confirm the Quill name and icon appear in the dock and the title bar.
   - Connect to a Postgres instance (Docker) and to a SQLite file. Ask "top 5 customers by revenue last year" and check the SQL, the grid and the bar chart.
   - Ask for a DELETE and check that the approval card blocks it.
   - Rename a column and check that a re-index happens.
5. `pnpm tauri build` produces a signed-off local bundle that launches.
