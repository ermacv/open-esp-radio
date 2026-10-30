# Durable host files

`oer-os-durable` owns the file primitives every repository tool shares:

- `atomic_write` and `atomic_json` replace a file atomically (a temporary
  file in the same directory, synced, renamed, then the directory synced);
- `atomic_write_private` uses the same durable sequence with a mode-0600
  temporary and replacement; callers own directory permissions;
- `sha256_file` and `sha256_bytes`, lowercase hex; `digests::DigestCache`
  remembers a file's digest by its identity and times (device, inode, size,
  modification and status-change time) in a cache file, so an immutable
  file is hashed once across processes;
- `unix_millis` and `unix_seconds`, wall-clock time;
- `xdg`: the user's state directories. Every cache, store, lock directory and
  configuration a tool keeps outside a checkout lies below
  `open-esp-radio/` of `$XDG_CACHE_HOME`, `$XDG_DATA_HOME`,
  `$XDG_CONFIG_HOME` or `$XDG_STATE_HOME` (state a program must not lose,
  such as the LAB service's), an empty variable counting as unset, through
  `xdg::path` or, where a tool has its own override variable,
  `xdg::overridable`.

It depends only on `serde`, `serde_json` and `sha2`.

```console
cargo test -p oer-os-durable
```
