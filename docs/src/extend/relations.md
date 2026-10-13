# Related Records

A resource lists another entity's records that belong to each of its own, as a
section on the edit page. A has-many section has New, Edit and Delete; a
many-to-many section has Attach, Edit and Remove.

```yaml
# descriptors/products.yaml
relations:
  variants:
    label: Variant
    entity: product_variants
    foreign_key: product_id
    list:
      columns:
        sku: {}
        price: { type: money }
    form:
      fields:
        sku: { rules: [required] }
        price: { type: money }
```

```rust
Resource::new("/products", "Products", list)
    .form(form)
    .permission("acme.manage_products")
    .relation(RelationConfig::has_many("variants", "Variant", "product_variants", "product_id", variants_list, variants_form))
```

```yaml
# descriptors/products.yaml
relations:
  tags:
    label: Tag
    entity: tags
    pivot:
      table: product_tags
      parent_key: product_id
      related_key: tag_id
      source: acme.tags
    list:
      columns:
        name: {}
    form:
      fields:
        note: {}
```

```rust
Resource::new("/products", "Products", list)
    .form(form)
    .permission("acme.manage_products")
    .relation(
        RelationConfig::belongs_to_many(
            "tags",
            "Tag",
            "tags",
            PivotConfig::new("product_tags", "product_id", "tag_id", "acme.tags"),
            tags_list,
        )
        .form(note_form),
    )
```

```rust
// The picker source Attach searches, registered with the admin.
Contributions {
    picker_sources: vec![PickerSourceReg::new(
        "acme.tags",
        Arc::new(TableSource::new("tags", "id", "name")),
    )],
    ..Default::default()
}
```

Key | Default | Does
--- | --- | ---
`label` | required | One related record, singular: the New or Attach button, the child form's title.
`title` | the name, humanized | The section heading.
`entity` | required | The related table.
`foreign_key` | one of the two | Has-many: the column on the related table holding the parent's id. Written by the framework from the URL; a field of that name in `form` is kept hidden.
`pivot` | one of the two | Many-to-many: the table joining the two, keys below.
`list` | required | The section's list: the same keys as a resource's `list`. Its columns are the related table's.
`form` | required for has-many | Has-many: the related record's form. Many-to-many: the fields kept on the pivot row, asked for on Attach; empty gives the section no Edit.
`permission` | the parent's | A permission the operator must also hold.

Pivot key | Default | Does
--- | --- | ---
`table` | required | The pivot table.
`parent_key` | required | The column holding the parent's id. Written by the framework from the URL.
`related_key` | required | The column holding the related record's id. Chosen in the picker on Attach, fixed afterwards.
`source` | required | The registered picker source Attach searches.
`id_field` | `id` | The pivot row's id column.

Path | Does
--- | ---
`/products/7/edit` | The parent's form, then a section per relation.
`/products/7/variants` | The section's list on its own, sorted and paged.
`/products/7/variants/new` | Has-many: a new related record. Many-to-many: Attach, a record from the picker with the pivot fields. Saves return to the parent.
`/products/7/variants/3/edit` | Has-many: the related record `3`. Many-to-many: pivot row `3`'s fields. Only under its own parent.
`/products/7/variants/delete` | Has-many: deletes the selected records through the related entity's listeners and persister. Many-to-many: deletes the selected pivot rows the same way; the records stay.

A relation needs a form on the parent. A relation's name is a path segment and
cannot be `new`, `edit`, `delete`, `import`, `export` or `columns`. A create
page names the sections it will have; "Save and continue editing" lands on
them. The section is never offered for a parent that does not exist. Attaching
a record already attached, or one the related table does not hold, is refused
with a message.
