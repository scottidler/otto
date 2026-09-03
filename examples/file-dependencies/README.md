# Example 8: File Dependencies Demo

This example demonstrates:

1. **File Dependencies**: Tasks that depend on input files using the `input` field
2. **Task Dependencies**: Tasks that depend on other tasks using the `before` field
3. **Combined Dependencies**: Tasks that have both file and task dependencies
4. **Glob Patterns**: File dependencies using wildcards like `*.txt` and `**/*.log`

## Structure

- `count_lines`: Depends on `data/*.txt` and `config.json`
- `summarize`: Depends on `README.md` and `data/summary.txt`
- `process`: Depends on both files (`process_config.yml`) AND tasks (`count_lines`, `summarize`)
- `check_logs`: Demonstrates glob patterns for log files

## Testing

Run the tasks to see file dependencies in action:
```bash
otto -o examples/file-dependencies/otto.yml process
otto -o examples/file-dependencies/otto.yml check_logs
```
