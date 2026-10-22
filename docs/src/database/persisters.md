# Persisters

A persister replaces the generic insert and update for one entity.

## Write a persister

```rust
#[async_trait]
impl Persister for NodePersister {
    async fn create(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<i64, SaveError>;
    async fn update(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), SaveError>;
    async fn load(&self, cx: &mut SaveCx<'_>, id: &str) -> Result<Option<Record>, SaveError>;
    async fn delete(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), DeleteError>;
}
```

Method | Default
--- | ---
`create` | Required.
`update` | Required.
`load` | `None`. The pre-write row, for listeners.
`delete` | Refuses.

`cx` carries the open transaction. Run every statement on it; never open your
own.

## Register it

```rust
registry.add_persister(PersisterReg::new("acme.tree", Arc::new(NodePersister)));

FormConfig::new("tree_node", "Node", "/nodes", "id", fields)
    .persist("acme.tree")
```

```yaml
# descriptors/nodes.yaml
form:
  persist: acme.tree
```
