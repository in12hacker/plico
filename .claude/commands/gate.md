Run the full quality gate for this project. Execute in order:

1. `EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib` — unit tests
2. `cargo clippy -- -D warnings` — lint check
3. `cargo fmt --check` — format check

Report each step's result. If any step fails, diagnose the issue and suggest fixes. Do NOT auto-fix — just report.