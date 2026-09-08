# Audit Log

Every administrative change is recorded in an append-only audit log, so there is
a durable record of who changed what, and when. The log is written automatically:
handlers do not opt in per action.

## What is recorded

Each entry captures:

- **When** the change happened.
- **The operator** who made it. The username is stored on the entry itself, so a
  record stays legible even after that account is later removed. A change made by
  the system rather than a person, from the command line or a background job,
  records the process name and no operator.
- **The action**, as a dot-keyed name such as `backend.role.update` or
  `backend.plugin.disable`.
- **The target** it acted on (a type and id), when the action has one.

The changes that are recorded are the ones that affect privileges or data:
creating and editing roles, changing a user's permissions, enabling or disabling
a plugin, saving a settings model, and creating or editing records through a
resource's form.

A settings value can hold secrets, so its entry records that the settings model
changed, not the new contents.

Writes through a resource's form are recorded by a [model
listener](model-listeners.md), so a screen you build from descriptors is audited
without doing anything. The framework's own role, user, plugin and settings
screens write through their own stores and record their entries directly.

## What is not recorded

An operator's own preference changes, their display timezone and interface
language, are personal and affect no one else, so they are not audited.

## Viewing the log

The log is a screen under **Settings → System → Audit Log**, listing entries
newest first. It is read-only: entries can be reviewed but never edited or
deleted from the admin.

Access is gated by the **View the audit log** permission
(`backend.view_audit_log`). Grant it to a role from the role editor; superusers
hold it already.

## For your own resources

A resource built the standard way (a `Resource` with a form) is audited with no
extra work: its create and edit actions are recorded as
`backend.<entity>.create` and `backend.<entity>.update`, attributed to the
signed-in operator.

## Reliability

The audit write happens after the change it records has committed. If that write
fails, the failure is logged for investigation but the operator's action is not
rolled back, so a logging problem never blocks legitimate work.
