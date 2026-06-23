#!/usr/bin/env bash
# Idempotent import of the already-live Zone-1 DO resources into Tofu state.
# Safe to re-run: `tofu import` no-ops a resource that's already in state.
# Resource IDs are the live ones from the 2026-06-22 doctl inventory.
set -uo pipefail
cd "$(dirname "$0")"
[ -n "${DIGITALOCEAN_TOKEN:-}" ] || { echo "source ./env.sh first" >&2; exit 1; }

imp() { tofu state show "$1" >/dev/null 2>&1 && echo "  have $1" || tofu import "$1" "$2"; }

# SSH keys (id from `doctl compute ssh-key list`)
imp digitalocean_ssh_key.mackes_mesh_claude 57249387
imp digitalocean_ssh_key.unit               57026644
imp digitalocean_ssh_key.eagle              57026397

# DNS zone + records (record import id = "domain,recordID")
imp digitalocean_domain.primary             matthewmackes.com
imp digitalocean_record.apex                matthewmackes.com,1813076736
imp digitalocean_record.lighthouse_01       matthewmackes.com,1822399094
imp digitalocean_record.lighthouse_02       matthewmackes.com,1822399050
imp digitalocean_record.lighthouse_03       matthewmackes.com,1822933937
imp digitalocean_record.voip                matthewmackes.com,1822399546

# Droplets
imp digitalocean_droplet.lighthouse_01      579112110
imp digitalocean_droplet.asterisk           440041476

echo "== import pass complete; run 'tofu plan' to verify clean =="
