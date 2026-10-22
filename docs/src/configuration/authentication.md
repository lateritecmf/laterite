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

[auth.password_policy]
min_length = 8
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
`password_policy.min_length` | The shortest password accepted, wherever one is set. Default `8`.

## Clear a lockout

```bash
lat admin unlock editor
```
