# WL-CRIT-006 farm sync capacity — 2026-08-09

`xcp-build.sh` now checks the selected build VM's `/home` free space before
rsync. The default minimum is 8 GiB; malformed measurements and insufficient
capacity fail before a partial slot is created. The helper never deletes farm
content automatically and names the bounded operator recovery.

## Machine 196 proof

- Build VM: `172.20.0.196`; slot `farm-space-preflight-r1`.
- With a deterministic `999999999` KiB requirement, the helper refused while
  reporting the observed `22030264` KiB and did not start rsync.
- With the default 8-GiB requirement, the same selected host admitted and
  completed the source sync.
- Exact shell syntax passed before both probes.
- Helper SHA-256:
  `38e83b43b6c3035ef26d925ca5ee108751cb34b41211e55f4a884d8f634dc661`.

Earlier in this run, three explicitly named abandoned farm source/build copies
were removed from machine 196, restoring `/home` from 95% to 66%. No source of
record or unrelated home data was removed.
