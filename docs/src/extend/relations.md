# Related Records

A resource lists another entity's records that belong to each of its own, as a
section on the edit page, with their own New, Edit and Delete.

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

Key | Default | Does
--- | --- | ---
`label` | required | One related record, singular: the New button, the child form's title.
`title` | the name, humanized | The section heading.
`entity` | required | The related table.
`foreign_key` | required | The column on the related table holding the parent's id. Written by the framework from the URL; a field of that name in `form` is kept hidden.
`list` | required | The section's list: the same keys as a resource's `list`.
`form` | required | The related record's form: the same keys as a resource's `form`.
`permission` | the parent's | A permission the operator must also hold.

Path | Does
--- | ---
`/products/7/edit` | The parent's form, then a section per relation.
`/products/7/variants` | The section's list on its own, sorted and paged.
`/products/7/variants/new` | A new related record; saves return to the parent.
`/products/7/variants/3/edit` | An existing one; only under its own parent.
`/products/7/variants/delete` | Removes the selected rows through the related entity's listeners and persister.

A relation needs a form on the parent. A relation's name is a path segment and
cannot be `new`, `edit`, `delete`, `import`, `export` or `columns`. A create
page names the sections it will have; "Save and continue editing" lands on
them. The section is never offered for a parent that does not exist.
