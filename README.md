<div align="center">
  <img src="assets/brand/quill-icon.svg" width="128" alt="Quill" />
  <h1>Quill</h1>
  <p><strong>Write your data in plain words.</strong></p>
  <p>An AI-native desktop database client for 90+ databases. Ask a question, and Quill reads your schema, writes the SQL, runs it safely, and answers with a table and a chart.</p>
</div>

## Why Quill

- **Ask in plain English.** Pick a connection and ask "top 5 customers by revenue last year". Quill finds the relevant tables, writes dialect-correct SQL, validates it, and shows the result.
- **Works with every kind of database.** PostgreSQL, MySQL, SQL Server, Oracle, SQLite, ClickHouse, DuckDB, MongoDB, Redis, Elasticsearch, and 80+ more through native, JDBC and agent drivers.
- **Safe by default.** AI-generated queries are read-only unless you allow writes per connection. Every write is classified for risk and needs explicit approval.
- **A full client underneath.** Query editor, data grid, ER diagrams, schema browser, import/export, SSH tunnels, explain plans, and an MCP server.

See [PLAN.md](PLAN.md) for the roadmap behind the Ask engine (schema retrieval, schema linking, validate-and-repair, chart selection, verified-query memory).

## Development

Requires Node.js 22.13+, pnpm 10, and a Rust toolchain.

```sh
pnpm install
pnpm dev:tauri        # desktop app
pnpm test             # frontend tests
pnpm typecheck
```

## License and credits

Quill is licensed under the Apache License 2.0. It is built on [DBX](https://github.com/t8y2/dbx) (Apache-2.0) and adapts ideas from [DB-GPT](https://github.com/eosphoros-ai/DB-GPT) (MIT). See [NOTICE](NOTICE).
