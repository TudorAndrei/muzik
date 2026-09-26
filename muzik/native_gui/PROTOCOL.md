# Native GUI protocol

The GPUI app starts `python -m muzik.native_gui` as a child process. It writes
one UTF-8 JSON object per line to standard input. The service writes one JSON
object per line to standard output. Only protocol records go to standard
output. The service ends when standard input closes.

## Request and response

```json
{"id":"42","command":"workflow.start","params":{"raw":"https://example.test/audio"}}
{"id":"42","type":"response","ok":true,"result":{"job_id":"..."}}
```

The `id` is copied into the response. An error response has `ok:false` and an
`error` object with `code` and `message`. Current codes are `invalid_request`,
`job_active`, and `operation_failed`. One job can run at a time. Job event
records can arrive before the response to a start command.

## Commands

| Command | Params | Result |
| --- | --- | --- |
| `hello` | none | `protocol_version`, launcher `defaults`, `item_actions` |
| `workflow.start` | `raw` and optional launcher fields | `job_id` |
| `job.cancel` | `job_id` | `cancel_requested` |
| `services.check` | none | `services` array with `name`, `available`, `detail`, `optional` |
| `library.scan` | optional `output` path | `output`, `total_size`, `items` array with size and modified time |
| `watchlist.load` | optional launcher fields | `watchlist` object |
| `watchlist.add` | `url` | `playlist`, `watchlist` |
| `watchlist.rename` | `playlist_id`, `title` | `renamed`, `watchlist` |
| `watchlist.remove` | `playlist_id` | `removed`, `watchlist` |
| `thumbnails.cache` | `video_ids` array of at most 16 IDs | `job_id` |
| `watchlist.refresh` | optional launcher fields | `job_id` |
| `watchlist.action` | `playlist_id`, `position`, `video_id`, `action`, optional launcher fields | `job_id` |
| `decision.reply` | `decision_id`, `value` | `decision_id` |
| `spotify.status` | none | `client_id`, `redirect_uri`, `connected`, optional `account_name` or `error` |
| `spotify.set_client_id` | `client_id` | `client_id` |
| `spotify.login` | optional `port` | `job_id` |
| `spotify.logout` | none | `removed` |
| `spotify.playlists` | none | `playlists` array |

Launcher fields match `WorkflowOptions`: `review`, `no_split`, `no_organize`,
`import_`, `tag_only`, `dry_run`, `jobs`, `config`, `keep_source`, `force`,
`metadata_source`, `audio_source`, `prefer`, `fallback`, `interactive`,
`quality_policy`, and `min_bitrate`. `raw`, `output`, and `splits` form the
workflow request. `hello` returns the main defaults. A missing optional field
uses the Python core default. `watchlist.add` accepts a YouTube or Spotify
playlist link, a Spotify album link, or `liked`. Each item in a returned
`watchlist` has `summary`, `thumbnail_path`, `primary_action`, and `actions`.
`primary_action` has an `action` and a `label`, or it is `null`. `actions` maps
each action name to `enabled` and `reason`. A `thumbnail_path` is a local path
or `null`. `thumbnails.cache` downloads missing card images for the requested
IDs. The app sends IDs from the current watchlist page. Its
`job.completed` result has `thumbnails` and the updated `watchlist`.

## Events

```json
{"type":"event","event":"job.event","data":{"job_id":"...","source":"workflow","event":"progress_started","data":{"task_id":"download","description":"Download","total":10}}}
{"type":"event","event":"job.completed","data":{"job_id":"...","result":{}}}
```

`job.event` carries a core event. `source` is `workflow` or `beets`. The event
name uses snake case without the Python `Event` suffix. Its `data` contains
the public fields of that event. Paths become strings and enums become their
values. Terminal events are `job.completed`, `job.failed`, and `job.cancelled`.
A completed watchlist job includes the new `watchlist` object. `job.failed`
has an `error` object. The GPUI app should reload the watchlist after a job
that changes it. `watchlist.load` returns saved cards at once. A worker then
checks local files and Beets. If the watchlist file did not change during that
check, the service saves the checked cards and sends `watchlist.updated` with
`{ "watchlist": ... }`. If another process changed the file, the worker reads
it again before it saves. If the check fails, the service sends
`watchlist.error` with a `message`. A load during a job does not start this
check. The service rejects add, rename, and remove commands until the job ends.

## Blocking decisions

The service keeps reading commands while a worker waits for a decision.

```json
{"type":"event","event":"decision.request","data":{"job_id":"...","decision_id":"...","kind":"chapter_review","payload":{"source":"/tmp/audio.m4a","chapters":[]}}}
{"id":"43","command":"decision.reply","params":{"decision_id":"...","value":"accept"}}
```

The decision kinds and reply values are:

| Kind | Reply value |
| --- | --- |
| `soulseek_candidate` | zero-based candidate index, or `{ "index": 0 }` |
| `chapter_review` | `accept`, `edit`, or `reject` |
| `chapter_edit` | a list of chapters with `index`, `start`, `end`, and `title`; `null` cancels the edit |
| `quality_replacement` | `true` or `false` |
| `beets_match` | one `candidate_id`, `as_is`, or `null` |
| `beets_duplicate` | `skip`, `keep_all`, `remove_old`, or `merge` |

`job.cancel` wakes a pending decision. Core operations check cancellation at
safe points. Spotify login opens the browser and waits for its loopback
callback. Its login wait has a timeout; `job.cancel` cannot stop that wait.
The service does not send OAuth tokens over the protocol.
