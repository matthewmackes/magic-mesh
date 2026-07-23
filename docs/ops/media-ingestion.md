# Media Ingestion

> **HISTORICAL / RETIRED (2026-07-23):** This runbook described the retired
> `Lighthouse_Media` topology. DigitalOcean lighthouses are thin control-plane
> nodes and must not host media or file-sharing services. Keep media workloads
> on a non-lighthouse host; do not use the commands below for a lighthouse.

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

The former Lighthouse_Media live verifier and promotion gate were removed on
2026-07-23. There is no supported media-lighthouse endpoint or verification
command. Keep any media workload on an explicitly provisioned non-lighthouse
host and verify that host with its own service checks.
