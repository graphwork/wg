# RED evidence — strong-agent merge resolution

Before this implementation (`main` at task start), the installed/API surface had no merge-resolution lane:

```text
$ git cat-file -e main:src/merge_resolution/mod.rs
fatal: path 'src/merge_resolution/mod.rs' does not exist in 'main'

$ git grep -n 'MergeResolution' main -- src/cli.rs src/main.rs
# no matches (exit 1)

$ git grep -n 'strong_agent_merge_resolution' main -- tests/smoke/manifest.toml
# no matches (exit 1)
```

The authoritative finalizer therefore converted every non-mechanical merge into `RepairNeeded` / `merge.conflict` or `merge.target_moved`; it had no exact strong-route snapshot, isolated integration repository, resolution descriptor, fresh descriptor-bound gates, or complete-resolution-tree CAS receipt. The classifier/fake-adapter and installed-binary smoke added by this task fail on that tree at command discovery before they can exercise any scenario step.
