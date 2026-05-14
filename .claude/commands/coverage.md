Run test coverage measurement:

```bash
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib
```

Report the coverage percentage. If below 90%, list the modules with lowest coverage and suggest what to test.