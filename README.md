# Sanity Log Explorer

A terminal UI for exploring Sanity request logs and spotting high-bandwidth assets. It parses NDJSON request logs that can be downloaded from the Sanity dashboard and provides a convenient interface for getting high-level stats on each asset.

## Features

- Parses NDJSON request logs with `body.url`, `requestSize`, and `responseSize` fields
- Aggregates by asset ID with request count, average size, and total bandwidth
- Alternate "By Type" view with extension breakdowns for images/files
- Sort by ID, extension, request count, average size, or bandwidth
- Open the selected asset URL in your system browser

<img width="912" height="740" alt="Screenshot 2026-01-12 at 7 43 55 PM" src="https://github.com/user-attachments/assets/99c3b0c1-455e-4720-a77d-592ef4816d03" />
<img width="912" height="740" alt="Screenshot 2026-01-12 at 7 44 04 PM" src="https://github.com/user-attachments/assets/1a161290-5686-46f3-b057-8b1adf26bed1" />
<img width="912" height="740" alt="Screenshot 2026-01-12 at 7 44 18 PM" src="https://github.com/user-attachments/assets/62fdd2ac-1b9a-499d-80e8-590bcc3353c4" />

## Install

Requires Rust. Build the binary with:

```bash
cargo build --release
```

The binary will be at `target/release/sanity-log-explorer`. You can copy it into your PATH for easier access.

## Usage

```bash
sanity-log-explorer <path-to-log.ndjson>
```

## Controls

- `↑/↓` or `j/k`: move selection
- `←/→` or `h/l`: switch tabs
- `Enter`: open selected asset URL
- `q` or `⌃C`: quit
- `?`: open help

Columns can be sorted by using the underlined character as a shortcut. Sorting again by the current column toggles ascending/descending order.

- `d`: sort by ID
- `e`: sort by extension
- `r`: sort by requests
- `s`: sort by size (avg)
- `b`: sort by bandwidth

## Input format

The app expects one JSON object per line (NDJSON). It looks for:

- `body.url` (string)
- `body.requestSize` (bytes, optional)
- `body.responseSize` (bytes, optional)

Paths are interpreted as:

- Images: `/images/:projectId/:dataset/:id-:dimensions.:ext`
- Files: `/files/:projectId/:dataset/:id.:ext`
- Queries: `/:version/data/query/:dataset`

## Notes

- Average request size is computed as total bandwidth divided by total requests.
- Opening a URL uses `open` (macOS), `xdg-open` (Linux), or `cmd /C start` (Windows).
