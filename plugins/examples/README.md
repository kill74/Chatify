# Plugin Examples

These examples document the API v1 shapes used by external plugin workers.

- `hello-world`: smallest useful slash-command plugin.
- `message-linter`: message hook that blocks obvious policy violations.

The files here are reference templates. Production plugins should be built as
executables and installed with:

```text
/plugin install /absolute/path/to/plugin-worker
```

