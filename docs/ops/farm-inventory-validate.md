# RUNBOOK — Validate the Xen farm inventory & finish the single-source-of-truth fix

> **Run this from Claude Code ON the dev host `172.20.145.192`** (Rocky 9, on the
> `172.20.x` LAN, with the tofu state, the mesh SSH key, and XCP-ng reachability).
> A cloud/web Claude session CANNOT do this — it has no route to the private LAN
> (the agent proxy denies private destination IPs). This runbook is the handoff.
>
> **Goal:** stop losing the Xen hosts + build slots on every context-clear by making
> **OpenTofu the single source of truth** that everything reads — and confirm the
> real farm against the live LAN first. The tool `install-helpers/farm-inventory.sh`
> (already on this branch) is that single source; this runbook validates it live and
> finishes wiring the rest of the repo to it.
>
> **How to run:** open this repo in Claude Code on the dev host, on branch
> `claude/cosmic-magic-mesh-egui-hs4n8j`, and say: *"Execute docs/ops/farm-inventory-validate.md."*

## Background (why this exists)

- Tofu already knows the farm: the **canonical root is `infra/tofu/xen-xapi`** (the
  XAPI-native root the reconciler uses), declaring **4 dom0s** —
  XEN-HOME-SERVICES (`.50`), KVM-XCP1 (`.90`), XEN-BIGBOY (`.130`), and
  **xen-194** (`.170`, the one the old docs/scripts missed).
- But nothing human/AI-facing read tofu — `farm.sh` hardcoded a **stale, dead**
  `.50/.51/.52`, `drain-coordinator.sh` hardcoded "7 slots / 3 nodes",
  `BUILD-ENVIRONMENT.md` listed 3 hosts, and the `ship`/`polish` skills carried
  their own stale tables. Every context-clear, Claude read one of those drifted
  copies instead of tofu.
- `install-helpers/farm-inventory.sh` is the fix: one tofu-derived command. This
  runbook proves it against the live LAN, then points the stragglers at it.

## Step 0 — Preconditions (verify, don't assume)

```sh
cd /root/magic-mesh   # or wherever the checkout lives on the dev host
git fetch origin && git checkout claude/cosmic-magic-mesh-egui-hs4n8j && git pull
test -f install-helpers/farm-inventory.sh && echo "tool present"
test -d infra/tofu/xen-xapi && echo "canonical tofu root present"
```

- Confirm you are on the LAN: `ip -4 addr | grep 172.20.` should show this host's
  address. If it doesn't, STOP — you're not on the dev host.

## Step 1 — Prove the tool (pure, no network)

```sh
./install-helpers/farm-inventory.sh selftest
```
**Acceptance:** prints `SELFTEST: PASS (4 dom0s, IPs + 9 heavy slots …)`. If it
fails, the tofu cold facts in `infra/tofu/xen-xapi/build-vms.tf` changed — fix the
tool's expectations to match tofu (tofu wins), don't hardcode around it.

## Step 2 — THE SCAN (the live host discovery you asked for)

Export the dom0 root password into the environment **only for this shell** — never
paste it into a file, a commit, or the chat:

```sh
read -rs XCP_PASS && export XCP_PASS          # type the dom0 root password, hidden
command -v sshpass >/dev/null || sudo dnf install -y sshpass   # needed for the SSH confirm
./install-helpers/farm-inventory.sh discover 172.20.145.190-198
unset XCP_PASS                                 # drop it from the env immediately after
```
This sweeps `172.20.145.190–198` for SSH(22)+XAPI(443), and for each SSH-reachable
host runs `xe host-list` (confirming it's genuinely XCP-ng) and prints its name.

**Acceptance / decide:**
- It should find **exactly the known dom0 hosts** — `172.20.145.165` (XEN-BIGBOY),
  `172.20.145.193` (KVM-XCP1), `172.20.145.194` (xen-194). (XEN-HOME-SERVICES is on
  `172.20.0.9`, outside this range; add `172.20.0.9` to a second sweep if you want
  it covered: `farm-inventory.sh discover 172.20.0.1-9`.)
- **If discover finds an XCP-ng host NOT in the declared fleet** → that's an
  undeclared dom0. Go to Step 4a to add it to tofu (this is the "there should be
  more" case made real).
- **If only the known hosts appear** → the 4-dom0 model is confirmed; skip to Step 5.

> SECURITY: rotate the dom0 `root` password after this run if it was shared in a
> chat transcript. The runbook never persists it.

## Step 3 — Cross-check against LIVE tofu state

```sh
cd infra/tofu/xen-xapi && source env.sh 2>/dev/null; tofu output -json build_farm | jq .
cd - >/dev/null
./install-helpers/farm-inventory.sh topology      # live probe: reachable? toolchained? free slots?
```
**Acceptance:** `tofu output build_farm` lists the running build VMs; `topology`
shows each declared dom0, whether its build VM is `up`+`ready`, and `N free / 9
total` heavy slots. A **declared-but-`down`** VM = under-provisioned capacity you're
not using → `farm.sh up` (Step 4b). A **reachable VM not in tofu** = drift → import
or destroy it so tofu stays authoritative.

## Step 4 — Reconcile any drift (only if Step 2/3 found some)

**4a. An undeclared dom0** (discover found an XCP-ng host tofu doesn't know):
add it to `infra/tofu/xen-xapi/build-vms.tf` `local.dom0` (copy an existing entry:
`provider_alias`, `network_uuid` from `xe network-list`, a fresh `ip_base` on a new
40-wide lane, `big_vcpus`/`big_mem_gib` from the host's capacity), add its
`provider_alias` block per the file's pattern, then add its mgmt IP + label to
`dom0_host_ip()`/`dom0_label()` in `farm-inventory.sh`. Re-run `selftest` — it
should now report the new count. Then `tofu plan` to converge.

**4b. A declared build VM is down:** `XCP_PASS=… ./install-helpers/farm.sh up`
(idempotent: keys the dom0, provisions/boots the VM, installs the toolchain).

## Step 5 — Finish the de-hardcoding (point every straggler at the one source)

Make these edits, then verify (Step 6). Each replaces a hardcoded/stale copy with a
read of `farm-inventory.sh`:

1. **`install-helpers/farm.sh`** — replace the stale `FLEET_DEFAULT` (`.50/.51/.52`)
   and `fleet()` so the fleet comes from the inventory tool:
   ```sh
   fleet() {
     [ -f "$CONF" ] && { grep -vE '^\s*(#|$)' "$CONF"; return; }
     # Single source of truth: derive host|label|buildvm from tofu via farm-inventory.
     "$HERE/farm-inventory.sh" fleet | awk -F'|' '{print $1"|"$2"|"$3}'
   }
   ```
   and update the header comment block (lines ~16–20) to the 4-dom0 reality.

2. **`install-helpers/drain-coordinator.sh`** — replace the hardcoded
   `NODES`/`NAMES`/`CAPS` arrays and the "7 slots" header with values read from
   `farm-inventory.sh fleet` (build-VM last octet, label, cap). Total becomes **9**.

3. **`install-helpers/xcp-build.sh`** — set `TOFU_DIR` default to the canonical
   `infra/tofu/xen-xapi` root (it currently defaults to the legacy 3-dom0 root), and
   add the `xen-194 → 172.20.0.170` case to `topology_from_tfvars()`'s fallback.

4. **`docs/BUILD-ENVIRONMENT.md`** — add the **xen-194** row to the §3 hardware
   table (`172.20.145.194` dom0 → build VM `mcnf-build-53` @ `172.20.0.170`), and in
   §3 + §10 add: *"For the live farm inventory, run
   `install-helpers/farm-inventory.sh topology` — do not memorize the host list."*

5. **`AI_GOVERNANCE.md` §10** — add one line: the canonical way to learn the farm
   topology is `install-helpers/farm-inventory.sh topology` (tofu-derived), not memory.

6. **`.claude/skills/ship/SKILL.md` + `.claude/skills/polish/SKILL.md`** — replace
   the embedded `.50/.51/.52` topology tables with a pointer:
   *"Farm topology is tofu-derived — run `install-helpers/farm-inventory.sh topology`
   (and `… fleet` for machine-readable). Do not hardcode IPs here."* *(Operator-gated
   skill config — make the edit, it's authorized by this runbook.)*

## Step 6 — Verify & commit

```sh
./install-helpers/farm-inventory.sh selftest                 # PASS
./install-helpers/farm-inventory.sh topology                 # live, sane, 9 slots
grep -rn '172\.20\.0\.5[12]' install-helpers/ .claude/ docs/ # MUST be empty (no stale .51/.52)
bash -n install-helpers/farm.sh install-helpers/drain-coordinator.sh install-helpers/xcp-build.sh
./install-helpers/farm.sh status                             # probes the REAL .50/.90/.130/.170
```
**Acceptance:** selftest PASS; the stale-IP grep is empty; `farm.sh status` reaches
the real build VMs (not dead `.51/.52`); `drain-coordinator.sh slots` totals 9.

Then commit on the same branch:
```sh
git add -A && git commit -m "farm: single tofu-derived inventory; de-hardcode farm topology (xen-194, 9 slots)"
git push -u origin claude/cosmic-magic-mesh-egui-hs4n8j
```

## Done = the context-clear problem is solved

After this, a fresh Claude context (or you) learns the farm by running ONE
tofu-derived command — `install-helpers/farm-inventory.sh topology` — instead of
trusting a memorized/drifted host list. The docs, skills, `farm.sh`, and the drain
coordinator all read that one source, so they can't drift from tofu again.
