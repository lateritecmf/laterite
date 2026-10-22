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
