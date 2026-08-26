# Friendly Local Domains

By default a Laterite app is reached at `http://127.0.0.1:8080`. For a friendlier
address like `http://acme.test:8080`, `lat domain` sets up wildcard DNS so every
host under a reserved `.test` TLD resolves to loopback. It is a development
convenience on macOS, built on `dnsmasq`.

This is the DNS layer only: the port stays in the URL. Dropping the port and
serving HTTPS (so `https://acme.test` works with no port) is a separate concern
handled by a reverse proxy, added later.

## Setup

```bash
brew install dnsmasq      # once, if not already installed
lat domain setup          # routes *.test -> 127.0.0.1
```

`setup` writes a `dnsmasq` wildcard rule and a `/etc/resolver/test` entry that
points the OS at `dnsmasq` for the TLD. The last two steps need administrator
rights, so it prompts for `sudo` as it runs. It is idempotent and skips anything
already in place. Pass `--dry-run` to see the plan without changing anything, and
`--tld <name>` to use a different reserved TLD.

Once set up, no per-app step is needed: serve any app and open it by name.

```bash
lat serve --port 8080
# open http://acme.test:8080/admin
```

Set `app.url` in the app's config to match, so absolute links (the startup
banner, later emails) use the friendly origin:

```toml
[app]
url = "http://acme.test:8080"
```

## Status and teardown

```bash
lat domain status         # reports whether *.test resolves to loopback
lat domain teardown       # removes what `lat domain` set up
```

`teardown` only undoes configuration `lat` created. If another tool (such as
Laravel Valet) already manages `.test`, `lat domain` detects it: `setup` will not
duplicate the rule, and `teardown` leaves the shared configuration untouched.
