# Friendly Local Domains

`lat domain` routes every host under a reserved `.test` TLD to loopback. macOS,
built on `dnsmasq`.

## Set up

```bash
brew install dnsmasq
lat domain setup
```

`setup` writes a `dnsmasq` wildcard rule and `/etc/resolver/test`, prompting
for `sudo`. It is idempotent.

Flag | Description
--- | ---
`--dry-run` | Print the plan and change nothing.
`--tld <name>` | Use a different reserved TLD.

## Open an application by name

```bash
lat serve --port 8080
# http://acme.test:8080/admin
```

Set `app.url` to match:

```toml
[app]
url = "http://acme.test:8080"
```

The port stays in the URL. HTTPS on the bare name is a reverse proxy's job.

## Status and teardown

```bash
lat domain status
lat domain teardown
```

`teardown` removes only what `lat domain` created. A `.test` managed by another
tool, such as Laravel Valet, is detected: `setup` adds nothing and `teardown`
leaves it alone.
