# Authentication

`[auth]` sets session lifetimes, sign-in lockouts and the password policy.
Every key is optional.

```toml
[auth]
session_idle_timeout_secs = 7200      # 2h
session_absolute_timeout_secs = 43200 # 12h
remember_duration_secs = 1209600      # 14d
max_failures = 5
max_failures_per_address = 20
failure_window_secs = 900
access_log_retention_days = 365

[auth.password_policy]
min_length = 12
require_uppercase = true
require_lowercase = true
require_number = true
require_nonalpha = true
expire_days = 90
allow_reset = true
reset_expire_minutes = 30
```

## Set the keys

Key | Description
--- | ---
`session_idle_timeout_secs` | Quiet time before a session ends, from the last request. Default `7200`.
`session_absolute_timeout_secs` | Ceiling from login, never extended. Default `43200`. The earlier name `session_ttl_secs` is still read.
`remember_duration_secs` | How long an unused "stay signed in" credential lasts, from its last use. Default `1209600`.
`max_failures` | Failed sign-ins from one address for one username before that address is locked out of it. Default `5`.
`max_failures_per_address` | Failed sign-ins from one address, any username, before the address is locked out. Default `20`.
`failure_window_secs` | Window the failures are counted over. Default `900`.
`access_log_retention_days` | Days of the access log the hourly purge keeps. Default `365`; `0` keeps every row.
`password_policy.min_length` | The shortest password accepted, wherever one is set. Default `8`.
`password_policy.require_uppercase` | A password needs an uppercase letter. Default `false`.
`password_policy.require_lowercase` | A password needs a lowercase letter. Default `false`.
`password_policy.require_number` | A password needs a digit. Default `false`.
`password_policy.require_nonalpha` | A password needs a character that is neither a letter nor a digit. Default `false`.
`password_policy.expire_days` | Days a password lasts; an older one opens nothing but the form that replaces it. Default `0`: no limit. An account that never changed its password ages from its creation.
`password_policy.allow_reset` | Whether the sign-in page offers "Forgot your password?". Default `true`; `false` also removes `/admin/forgot`.
`password_policy.reset_expire_minutes` | How long a reset link works. Default `60`, at least `1`.

Each password form lists the rules in force, and a refused password is told
every rule it breaks.

## Clear a lockout

```bash
lat admin unlock editor
```
