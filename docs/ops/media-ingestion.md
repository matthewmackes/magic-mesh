# Media Ingestion

MEDIA-9's operator path is `automation/media/ingest-music.sh`.

The script reads the leader-managed `media-spaces` secret, writes temporary
root-only `rclone` and `curl` config files, uploads a file or directory to the
shared DO Spaces bucket, then triggers Navidrome `startScan` on `music.mesh` and
any resolved `music.mesh` A-records.

```bash
automation/media/ingest-music.sh /path/to/album-or-track
```

Useful options:

```bash
automation/media/ingest-music.sh --dest-prefix music /path/to/album
automation/media/ingest-music.sh --skip-rescan /path/to/album
automation/media/ingest-music.sh --rescan-url http://10.42.0.20:4533 /path/to/album
```

The remaining live check is client-visible: after a Lighthouse_Media node is
serving, upload known tracks, wait for scan completion, then verify `mde-music`
can browse and stream them through `http://music.mesh:4533`.

## Live Verification

Use the verifier after at least one `Lighthouse_Media` node is serving:

```bash
automation/media/verify-media-lighthouse.sh
```

That non-mutating check reads the leader-managed `media-spaces` secret, verifies
`music.mesh` and `music-writer.mesh` DNS, and pings both Navidrome endpoints with
the shared account. For the MEDIA-6 playlist-state proof, arm a temporary
playlist write/read/delete against the deterministic writer endpoint:

```bash
automation/media/verify-media-lighthouse.sh --mutate-playlist
```

The promotion wrapper exposes the same gate as:

```bash
automation/promotion/mcnf-promotion-cycle.sh media-verify
automation/promotion/mcnf-promotion-cycle.sh media-verify --mutate-playlist
```
