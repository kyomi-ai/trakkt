# State beyond RAM

Store **durable state** with *bounded memory* and `RocksDB`.

## Storage plan

- Keep recent state in memory.
- Persist older state on disk.

1. Write a checkpoint.
2. Restore the checkpoint.

[Storage guide](https://example.com/storage)

| Layer | Retention | Key |
| --- | --- | --- |
| Memory | Recent | a_very_long_state_partition_key_that_requires_horizontal_space_in_a_narrow_project_layout |
| Disk | Durable | checkpoint |

```rust
let checkpoint = "a long checkpoint value that should scroll horizontally without stretching the project page";
restore(checkpoint);
```

> Recovery uses the last committed checkpoint.
