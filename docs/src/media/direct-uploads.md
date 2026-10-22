# Direct Uploads

A direct upload sends the bytes from the browser straight to a disk that
presigns, and the application records the file once it has landed.

```
POST /admin/media/direct            name=clip.mp4&content_type=video/mp4&size=52428800
→ {"direct": true, "url": "<presigned PUT>", "path": "quarantine/<id>/clip.mp4", "headers": {"Content-Type": "video/mp4"}}
PUT <presigned PUT>                 the bytes, from the browser
POST /admin/media/direct/complete   path=quarantine/<id>/clip.mp4&name=clip.mp4&folder=3
→ {"stored": [{"id": 7, "name": "clip.mp4", "href": "/admin/media/7", "url": "..."}], "refused": []}
```

Step | On a disk that presigns | On any other disk
--- | --- | ---
`direct` | A PUT URL under `quarantine/<32 hex>/<name>`, good for an hour. | `{"direct": false}`: the uploader posts the file instead.
`complete` | The object is checked, recorded and removed from `quarantine/`. | The same, for an object already under `quarantine/`.

## Turn it on

The upload zone does this on its own when it carries `upload_direct`, the URL
of the `direct` route (see [Receive an
upload](../services/storage-disks.md#receive-an-upload)). The library's own
upload carries it. A path outside `quarantine/`, or an object larger than the
upload limit, is refused.

Abandoned quarantine objects are the bucket's to expire: set a lifecycle rule
of one day on the `quarantine/` prefix.

## Presign from a driver

Method | Default | On `S3Disk`
--- | --- | ---
`StorageDriver::presigned_put(path, content_type, expires_in)` | `None` | A presigned PUT.
`StorageDriver::copy(from, to)` | `get` then `put` | A server-side copy.

A driver returning `None` from `presigned_put` answers `{"direct": false}`.
