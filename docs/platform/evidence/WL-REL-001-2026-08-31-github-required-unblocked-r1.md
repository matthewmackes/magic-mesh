# WL-REL-001 github-required unblocked — r1

Date: 2026-08-31  
Classification: dest-operator GitHub Actions enablement; **not** a
`github-required` pass, tag, or publication

## Cause

`GET /repos/matthewmackes/magic-mesh/actions/permissions` returned
`enabled: false`. Freeze-SHA dispatches stayed `queued` with **zero jobs**.
Runner list was empty. Hosted `ubuntu-latest` never started.

## Correction

1. Enabled Actions (`allowed_actions: all`). Freeze tree was not edited.
2. Registered self-hosted runner `mcnf-farm` with labels
   `self-hosted`, `Linux`, `X64`, `mcnf-farm` (systemd unit
   `actions.runner.matthewmackes-magic-mesh.mcnf-farm.service`).
3. Dispatched `ci.yml` on freeze SHA `42035dcbd` /
   https://github.com/matthewmackes/magic-mesh/actions/runs/33398044275
   Status: `in_progress`. Hosted jobs and `farm-gate` both started.

## Not done

`github-required` has not passed. Do not tag. Do not treat this run as
close of WL-REL-001 until that check is green on this SHA.
