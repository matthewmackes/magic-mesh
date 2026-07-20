# IAC — the "Infra as Code (IaC)" workspace (OpenStack IaaS control plane)

Operator-locked 2026-07-04 (25-Q survey). A new **Workloads-group** surface named
**"Infra as Code (IaC)"**: a comprehensive **OpenStack IaaS control plane** built on
OpenStack's own **standard APIs** — the **Keystone service catalog** is the service
directory, **OpenStack Heat** is the Infrastructure-as-Code engine, and each service's
**standard API** drives status + the full control surface. It lists every service
published on the network (OpenStack catalog + mesh/LAN scan, grouped), with a live
OpenStack-API status band on top.

## Framing (operator, 2026-07-04) — OpenStack is the standard API

The operator corrected an early tofu/ansible framing: **OpenStack already provides these
capabilities through its standard IaaS APIs.** So the workspace is OpenStack-native —
- the **service catalog** (Keystone) is the authoritative "what services exist" list,
- **Heat** (HOT templates + stacks) is the native "as Code" / orchestration engine,
- the per-service **standard APIs** provide status + every control.
No external IaC tool. This aligns with **CONSTRUCT-CLOUD** (`docs/design/quasar-cloud.md` —
the mesh becoming an OpenStack cloud). The **[[menubar-all]]** governing principle — *the
menu bar surfaces ALL controls, incl. advanced/complex* — lands hardest here: the menus
carry the full standard-API verb set.

## Locked decisions (25)

| # | Area | Lock |
|---|------|------|
| 1 | Scope | **Service directory + OpenStack API status + live IaC control** (Heat) — the full OpenStack IaaS control plane, comprehensive controls (the governing principle). |
| 2 | OS services | **Whatever the live Keystone catalog advertises** — a tile per advertised service endpoint (identity/nova/neutron/glance/cinder/placement/heat/designate/swift/octavia…), auto-adapting to the real deployment. |
| 3 | API tile | **Rich: health + latency + version + endpoints** — up/down (real API probe), latency, microversion, region, and the public/internal/admin URLs + listening port. |
| 4 | API actions | **Full per-service control (comprehensive)** — restart/reload, tail logs, view config, health/endpoint self-test, drill into the service's resources; destructive typed-armed. |
| 5 | IaC engine | **OpenStack Heat (native) + reverse-generate** — HOT templates + stacks via the standard Heat API; PLUS generate/export a HOT template from the live discovered infra (capture reality as code). No external tofu/ansible. |
| 6 | Heat ops | **Full Heat control loop** — list/show stacks, view + edit templates, preview-update (dry-run diff), create/update/delete (armed), stack-check (drift), events + resources drill. |
| 7 | Directory source | **OpenStack catalog + mesh scan merged** — Keystone catalog authoritative for cloud services; the mesh/mackesd catalog + LAN scan (`probe_nmap`, `descriptors`) fold in non-OpenStack services; one merged, de-duped directory. |
| 8 | Grouping model | **Same directory, grouped by kind** — an "OpenStack (IaaS)" group + "Mesh services" + "LAN" groups in one grouped list. |
| 9 | Service row | **Rich: endpoint + health + meta** — name/kind, endpoint (host:port/URL), health dot + latency, region/version (OpenStack), owner node, discovery source (catalog/scan/mesh). |
| 10 | Grouping detail | **By service type within top buckets** — OpenStack → Compute/Network/Image/Volume/Orchestration/Identity/DNS/Object; + Mesh + LAN buckets. |
| 11 | Row actions | **Comprehensive** — open/connect, health self-test, tail logs, drill into resources, management verbs (restart/reload, armed). |
| 12 | Freshness | **Live health + on-demand rescan** — catalog live from Keystone; health/latency auto-poll ~15s; a Rescan button re-runs the LAN/mesh scan. |
| 13 | Resource control | **Read + navigate tables; mutations via the menu bar** — drilling shows read-only resource tables; all mutating ops flow through the comprehensive menu bar (clean tables, deliberate actions — reinforces the [[menubar-all]] control-surface principle). |
| 14 | Coverage | **Every cataloged service, full CRUD** — catalog-driven; a newly-added OpenStack service automatically gets its resource panel + verbs. |
| 15 | Verb surfacing | **Resource tables + menu verbs + forms** — sortable tables with row + bulk selection; the full verb set in the menu bar; create/update open real forms (flavor/image/network pickers). |
| 16 | Relationships | **Linked resource view** — an instance → its ports/networks/volumes/floating-IPs/security-groups/Heat-stack, cross-linked + jumpable. |
| 17 | Menu structure | **Dynamic per-service menus from the catalog** — one menu per advertised service (Compute/Network/Image/Volume/Heat/Identity/DNS/…) carrying its full standard verb set, + Catalog/View/Help; the bar auto-grows with the catalog. |
| 18 | Power tools | **Menus + forms only** — the comprehensive menus + forms cover ops; a truly obscure op goes to the Terminal surface (no separate palette/raw-runner pane). |
| 19 | Context | **Single default context** — the mesh's cloud + the operator's project/region (implicit, no switcher). |
| 20 | Auth | **clouds.yaml on the node** — the openstacksdk standard (`~/.config/openstack/clouds.yaml`). |
| 21 | Layout | **Tabbed: Overview \| Resources \| Heat** — the API status band always on top; Overview (band + directory), Resources (per-service tables), Heat (stacks/templates/drift). |
| 22 | Arming | **Typed-arming on all mutations** — every mutating op (instance delete/rebuild/migrate, volume/network delete, stack update/delete, service restart) requires typed-arming (type the resource/stack name). |
| 23 | Audit/notify | **Audit-all + notify on failure/outage only** — every mutating op → the KDC hash-chained audit log; the mesh notify feed fires only on a failure or a service going down (not routine successes). |
| 24 | vs Instances | **IaC is the full admin; Instances stays the quick VM view** — the IaC workspace is the comprehensive OpenStack admin (all services); the existing Instances surface (QC-12) stays the focused "my VMs" broker; shared OpenStack client/data; IaC links to Instances for compute. |
| 25 | Phasing | **Everything in one cut** — all three tabs + the merged directory + comprehensive control land together. |

## Architecture

### The OpenStack client foundation (IAC-1)
A Rust OpenStack client layer authenticating via **clouds.yaml** (openstacksdk standard),
exposing: the **Keystone catalog** (service list + endpoints), per-service **API health**
(a real ping/version probe), and the **standard resource + verb calls** (Nova/Neutron/
Glance/Cinder/Heat/…). Reuse/extend the platform's existing OpenStack integration
(`mackesd` QC verbs / `ipc/datacenter` / `compute_registry` / the CONSTRUCT-CLOUD epic) rather
than a parallel client (§6). The catalog is the authoritative directory source.

### The surface — `Surface::InfraCode` (IAC-2..N) in `mde-shell-egui`
A new surface in the **Workloads** dock group (name "Infra as Code (IaC)"), using the
**[[menubar-all]] shared MenuBar** with **dynamic per-service catalog menus**:
- **Overview tab** — the **OpenStack API status band** (rich tiles from the live catalog:
  health/latency/version/endpoints, per-service actions) + the **merged service directory**
  (Keystone catalog + mesh/LAN scan), grouped by type within OpenStack/Mesh/LAN, rich rows,
  live health + Rescan.
- **Resources tab** — per-service **read-only resource tables** (sortable, row + bulk
  select) with the mutating **verbs in the menu bar** + **forms** (create/update) + the
  **linked cross-service view**; every cataloged service gets full CRUD.
- **Heat tab** — stacks + templates + **preview-update diff** + **stack-check drift** +
  events/resources + **reverse-generate** (live infra → HOT); the full IaC loop.
- **Typed-arming** on every mutation; **audit** every op; **notify** only on failure/outage.
- All look via `mde_egui` `Style`/`Motion` (§4); non-OpenStack mesh/LAN services fold into
  the same directory (from `descriptors`/`probe_nmap`/the mackesd media/service registries).

### Relationship to the platform
- **Instances (QC-12)** stays the quick VM broker; IaC is the full admin; both share the
  OpenStack client (IAC-1); IaC → Instances jump for the deep compute view (#24).
- **CONSTRUCT-CLOUD** provides the underlying cloud; IaC is its operator control plane.
- The **comprehensive menu bar** is the [[menubar-all]] governing-principle headline case.

## Acceptance (runtime-observable; per task — §7)
- The Workloads dock shows **"Infra as Code (IaC)"**; opening it shows the **OpenStack API
  status band** (rich tiles from the live Keystone catalog — health/latency/version/
  endpoints) over a **merged, grouped service directory** (OpenStack catalog + mesh/LAN
  scan), rich rows, live health + Rescan.
- **Resources tab**: drilling a service shows its real resource tables; **every mutating
  op flows through the catalog-driven menu bar** (full CRUD per service), forms open for
  create/update, the **linked view** cross-references instances↔ports/volumes/IPs/stacks —
  all **typed-armed**, **audited**, real OpenStack API calls (no mockups).
- **Heat tab**: real stacks/templates, **preview-update diff**, **stack-check drift**,
  **reverse-generate** a HOT template from live infra; create/update/delete armed.
- Auth via **clouds.yaml**; single default context; **notify fires on failure/outage**; the
  menu bar surfaces the **full standard-API verb set** (the governing principle).

## Risks
- **A comprehensive OpenStack admin is large** — even "one cut" (#25) is a big surface;
  IAC-1 (the client) + the catalog-driven Resources/verbs are the load-bearing effort.
  Build the client foundation first even though the tabs land together.
- **Live-destructive over the standard API** — deleting/rebuilding/migrating real cloud
  resources; typed-arming + audit + notify-on-failure are the guardrails; test against a
  throwaway project first (QC-16 verb CI).
- **Catalog-driven UI generality** — auto-generating menus/tables/forms from the catalog +
  microversions must degrade honestly when a service/verb/field is absent (§7), never a dead
  menu.
- **clouds.yaml on disk** (#20) is a credential file — keep it root/user-scoped; a later
  hardening could move to application-credentials (raised, not locked).
- **Two instance views** (IaC + Instances) must share one client + not diverge (#24).
- **OpenStack may be partially deployed** on the current mesh — every tile/panel must render
  honestly when a service is absent/unreachable (the catalog + probe drive presence).

## Out of scope (v1)
- External IaC tools (tofu/ansible) — Heat is the native IaC (#5); the platform's `infra/`
  tofu remains a separate provisioning concern, not surfaced here.
- Multi-cloud / multi-context switching (#19 single context; a selector is a later add).
- A command palette / raw CLI pane (#18 — the Terminal surface is the escape hatch).
- Application-credential auth (clouds.yaml first; app-creds a hardening follow-up).

## Tasks → `docs/WORKLIST.md` IAC-1 (OpenStack client foundation) + IAC-2..N (surface + tabs + directory + resources + Heat + audit/notify + live smoke). Depends on [[menubar-all]] MENUBAR-ALL-1 (shared MenuBar) for the dynamic catalog menus.
