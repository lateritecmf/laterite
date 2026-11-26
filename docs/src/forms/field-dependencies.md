# Field Dependencies

A field follows another as it is typed, or changes when another field's state
meets a condition.

```yaml
fields:
  title: {}
  slug:  { preset: title }
  path:  { preset: { field: title, type: url } }
  is_delayed: { type: switch, label: Send later }
  send_at:
    type: date
    trigger: { action: show, field: is_delayed, condition: checked }
  format: { type: select, options: [{ value: csv }, { value: xml }] }
  delimiter:
    trigger: { action: show|empty, field: format, condition: "value[csv]" }
  topics: { type: checklist, options: [{ value: news }, { value: sport }] }
  region:
    trigger: { action: enable, field: "topics[]", condition: "value[sport]" }
```

## Follow another field

Key | Value
--- | ---
`preset` | The field followed, or `{ field, type }`. Follows it as it is typed, until this field is edited; emptying this field hands it back.
`preset.type` | `slug` (default): `hello-world`. `url`: `/hello-world`. `file`: spaces to hyphens. `exact`: as typed.

## React to another field

Key | Value
--- | ---
`trigger.action` | `show`, `hide`, `enable`, `disable`, `empty`, `fill[value]`, several joined by `\|`. A hidden field is left out of the submission.
`trigger.field` | The field watched. `name[]` watches every ticked value of a checklist.
`trigger.condition` | `checked`, `unchecked`, `value[x]`, `value[x][y]` (either), `value[]` (empty), `value[*]` (anything), `value[csv*]` (a prefix).

A field that follows or watches one the form does not have stops the boot,
naming it. An action or condition the page does not know is refused when the
file is read.

## Options from a source

```yaml
fields:
  country:  { type: select, options: acme.countries }
  state:    { type: select, options: acme.states, depends_on: country }
  timezone: { type: select, options: laterite.timezones }
```

```rust
use laterite_admin::options::{OptionCx, OptionEntry, OptionError, OptionSource, OptionSourceReg};

struct States;

#[async_trait]
impl OptionSource for States {
    async fn options(&self, db: &Db, cx: &OptionCx<'_>) -> Result<Vec<OptionEntry>, OptionError> {
        let country = cx.values.get("country").map(String::as_str).unwrap_or("");
        Ok(states_of(db, country)
            .await?
            .into_iter()
            .map(|s| OptionEntry::new(s.code, s.name))
            .collect())
    }
}

registry.add_option_source(OptionSourceReg::new("acme.states", Arc::new(States)));
registry.add_option_source(OptionSourceReg::fixed(
    "acme.sizes",
    vec![OptionEntry::new("s", "Small"), OptionEntry::new("l", "Large").note("From 40 cm")],
));
```

Key | Value
--- | ---
`options` | On a `select`, `radio` or `checklist`: the name of a registered source, in place of a list.
`depends_on` | A field, or a list of fields. When one changes, this field is drawn again and its source is asked with the form's current values.

`OptionCx` | Holds
--- | ---
`field` | The field's name.
`value` | The field's current value.
`values` | Every value the form holds, by field name. For a list filter: every filter value, by filter name.
`id` | The record being edited, on an edit page.
`i18n` | The request's translator.

`OptionEntry` | Effect
--- | ---
`::new(value, label)` | A choice storing `value`, shown as `label`.
`.note(text)` | A second line under the label (radio, checklist).
`.disabled()` | Shown and not selectable (select, radio).
`.group(heading)` | Sits under `heading` (select, checklist).

Built-in source | Offers
--- | ---
`laterite.timezones` | Every timezone name.
`laterite.locales` | The languages the admin is translated into.

A source name nobody registered, a field depending on one the form does not
have, and a field depending on itself stop the boot. A row of a `repeater`
lists its options in place.
