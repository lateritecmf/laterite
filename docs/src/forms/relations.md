# Related Records

A resource lists another entity's records that belong to each of its own, as a
section on the edit page. A has-many section has New, Edit and Delete; a
many-to-many section has Attach, Edit and Remove.

## Declare a has-many relation

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

## Declare a many-to-many relation

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

## Declare a polymorphic relation

```yaml
# descriptors/notes.yaml, and the same in descriptors/tasks.yaml
relations:
  comments:
    label: Comment
    entity: comments
    morph: commentable
    list:
      columns:
        body: {}
    form:
      fields:
        body: { rules: [required] }
  tags:
    label: Tag
    entity: tags
    pivot:
      table: taggables
      parent_key: taggable_id
      parent_type: taggable_type
      related_key: tag_id
      source: acme.tags
    list:
      columns:
        name: {}
```

```rust
RelationConfig::has_many("comments", t!("Comment"), "comments", "", list, form).morph("commentable");
PivotConfig::new("taggables", "taggable_id", "tag_id", "acme.tags").parent_type("taggable_type");
```

Column | Holds
--- | ---
`commentable_id` | The parent's id
`commentable_type` | The parent's entity, such as `notes`
`taggable_type` | On the pivot: the parent's entity

The type columns are written from the URL and kept hidden in the form. A
section lists only rows whose type is its parent's entity.

## Edit a one-to-one record in the form

```yaml
# descriptors/authors.yaml
form:
  related:
    profile: { entity: profiles, foreign_key: author_id }
    address: { entity: addresses, belongs_to: address_id }
  fields:
    name: { rules: [required] }
    bio:  { related: profile }
    city: { related: address }
```

```rust
form.related.insert("profile".into(), RelatedRecord::has_one("profiles", "author_id"));
form.related.insert("address".into(), RelatedRecord::belongs_to("addresses", "address_id"));
```

`related` key | Holds
--- | ---
`entity` | The related table
`foreign_key` | Has-one: the related table's column holding this record's id
`belongs_to` | Belongs-to: this table's column holding the related record's id
`id_field` | The related table's id column; default `id`
`timestamps` | `true`: the related row's `created_at` and `updated_at` are stamped as it is written
`optional` | Belongs-to, `true`: with every field of it empty no record is made, and one already there is let go (this record's column is cleared, the related row stays)

Step of a save | Writes
--- | ---
1 | Each belongs-to record, inserted or updated; its id goes into this record's column
2 | This record
3 | Each has-one record, inserted or updated with this record's id
4 | The media links of this record's media fields

All four run in one transaction: a failure in any of them saves none. A field
with `related` cannot carry `unique`.

## Delete related rows with their parent

```yaml
# descriptors/notes.yaml
soft_delete: true
relations:
  comments:
    title: Comments
    entity: comments
    morph: commentable
    soft_delete: true
    delete: true
    section: false
```

```rust
RelationConfig::has_many("comments", t!("Comment"), "comments", "note_id", list, form)
    .delete()
    .section(false)
```

Parent | `soft_delete` | `delete`
--- | --- | ---
Moved to the trash | Rows go to the trash with it |
Restored | Rows that went with it come back |
Deleted, or deleted permanently from the trash | | Rows are deleted in the same transaction

A deleted row's own `delete` relations, declared on a resource over its
entity, go with it:

```yaml
# descriptors/comments.yaml
entity: comments
relations:
  reactions:
    title: Reactions
    entity: reactions
    foreign_key: comment_id
    delete: true
    section: false
```

Delete asks first and names each relation by its `title`:
"These are deleted with them: Comments, Reactions." The rows' own listeners do
not run.

## Set the keys

Key | Default | Does
--- | --- | ---
`label` | required | One related record, singular: the New or Attach button, the child form's title.
`title` | the name, humanized | The section heading.
`entity` | required | The related table.
`foreign_key` | one of three | Has-many: the column on the related table holding the parent's id. Written by the framework from the URL; a field of that name in `form` is kept hidden.
`morph` | one of three | Polymorphic has-many: `name` for the `name_id` and `name_type` columns.
`soft_delete` | `false` | Has-many: rows go to the trash with the parent and come back with it. See [Soft Delete](../database/soft-delete.md).
`delete` | `false` | Has-many: rows are deleted with the parent. See [Delete related rows with their parent](#delete-related-rows-with-their-parent).
`section` | `true` | `false` gives the relation no section and no pages; `soft_delete` and `delete` still apply, and `list` and `form` are not needed.
`pivot` | one of three | Many-to-many: the table joining the two, keys below.
`list` | required | The section's list: the same keys as a resource's `list`. Its columns are the related table's.
`form` | required for has-many | Has-many: the related record's form. Many-to-many: the fields kept on the pivot row, asked for on Attach; empty gives the section no Edit.
`permission` | the parent's | A permission the user must also hold.

Pivot key | Default | Does
--- | --- | ---
`table` | required | The pivot table.
`parent_key` | required | The column holding the parent's id. Written by the framework from the URL.
`related_key` | required | The column holding the related record's id. Chosen in the picker on Attach, fixed afterwards.
`source` | required | The registered picker source Attach searches.
`id_field` | `id` | The pivot row's id column.
`parent_type` | none | Polymorphic: the pivot column holding the parent's entity.

## Reach the section

Path | Does
--- | ---
`/products/7/edit` | The parent's form, then a section per relation.
`/products/7/variants` | The section's list on its own, sorted and paged.
`/products/7/variants/new` | Has-many: a new related record. Many-to-many: Attach, a record from the picker with the pivot fields. Saves return to the parent.
`/products/7/variants/3/edit` | Has-many: the related record `3`. Many-to-many: pivot row `3`'s fields. Only under its own parent.
`/products/7/variants/delete` | Has-many: deletes the selected records through the related entity's listeners and persister. Many-to-many: deletes the selected pivot rows the same way; the records stay.
`/products/7/variants/reorder` | Has-many with `reorder` on its `list`: moves one record within the parent's section. See [Trees and Reordering](../lists/structures.md).

A relation needs a form on the parent. A relation's name is a path segment and
cannot be `new`, `edit`, `delete`, `import`, `export` or `columns`. Attaching
a record already attached, or one the related table does not hold, is refused
with a message.
