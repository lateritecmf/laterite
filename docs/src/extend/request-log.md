# Request Log

Every URL the admin could not serve is counted, once per method, path and
status, with the first and last time it was seen.

```yaml
# descriptors/request-log.yaml, built in
entity: backend_request_log
path: /request-log
permission: backend.view_request_log
```

Column | Holds
--- | ---
Last seen, First seen | When the URL was last and first requested.
Method, Path | The request; the path is kept to 512 characters, without its query string.
Status | `404`.
Hits | How many times.

Behaviour | Does
--- | ---
Writing | The admin's 404 fallback counts the request before answering.
Viewing | Settings › System › Request Log, for an operator holding `backend.view_request_log`.
Clearing | Rows are selected and deleted like any list's; the log keeps the two thousand most recently seen on its own.
