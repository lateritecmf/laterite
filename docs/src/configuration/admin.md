# Admin

`[backend]` configures the admin panel. Every key is optional.

```toml
[backend]
secure_cookie = false            # true behind HTTPS
timezone = "UTC"                 # default display timezone, IANA name
path = "/admin"                  # where the admin mounts
trusted_proxies = []             # CIDR ranges whose X-Forwarded-For is believed
storage = "storage"              # where files the admin keeps go
secret = "a-long-random-string"  # signs timed URLs
skin = "standard"
upload_extensions = []

[backend.order]
"backend.nav" = ["dashboard", "acme.shop", "media", "settings"]

[backend.menu]
"backend.access_log" = { in = "backend.nav", slot = "tools" }

[backend.paths]
"acme.places" = "/places"          # a whole module's screens
"acme.places/nodes" = "/places"    # one screen; the more specific key wins
```

## Set the keys

Key | Description
--- | ---
`secure_cookie` | Sets the Secure flag on cookies and sends `Strict-Transport-Security` (one year, subdomains included). `true` behind HTTPS.
`timezone` | Default display timezone until a user picks their own. See [Dates and Timezones](dates-and-timezones.md).
`path` | URL path the admin mounts under.
`trusted_proxies` | CIDR ranges. `X-Forwarded-For` is read only from a peer inside one. Empty records the peer address.
`paths` | Remounts a module's screens, or one screen, at another path. Two claims on one path abort the boot.
`upload_extensions` | The file extensions an upload may carry, lowercased. Empty applies the default list (images, documents, audio, video, fonts, archives). See [Media Library](../media/library.md).
`skin` | The admin skin by code: `standard` (default), `compact`, or one a module registered. See [Skins](../admin/skins.md).
`storage` | The directory files the admin keeps go under, relative to the working directory unless absolute. Uploaded import files go to `private` beneath it.
`order` | Arrangement overrides by hierarchy: `"backend.nav"`, `"backend.nav.<section>"`, `"backend.settings"` or a plugin's own, each a list of keys in the wanted order. See [Navigation](../admin/navigation.md).
`menu` | Entries moved out of their home, keyed by entry: `{ in, slot, order, category }`. See [Navigation](../admin/navigation.md).
`secret` | The key timed URLs are signed with (a private file's). Set it when several instances share one database.
