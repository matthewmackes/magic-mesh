# WL-FUNC-019 S5 live Windows RDP authority check — 2026-08-09

## Outcome

Basement seat 15 still discovers `172.20.146.54:3389`, and a fresh bounded
seat-side TCP probe reached the endpoint. Authenticated connection/render proof
was not attempted because both required authority inputs fail closed:

- the authoritative mesh store has no `resource/publisher-hmac` record;
- the discovered RDP card has no credential reference, and no Windows username
  or password was supplied through the shell's operator prompt.

These are independent credentials. The publisher HMAC attests resource actions;
it is not a Windows login. The shell's production store cannot persist a generic
external desktop credential yet, so inventing one from the catalog would bypass
the explicit operator boundary.

## Governed distribution diagnosis

Seat 15 is a pinned Workstation with a root-only regular age identity. Its public
recipient is already registered by the packaged
`mcnf-mesh-secret-recipient.service`, whose last result is successful. Repository
governance requires an existing Lighthouse holder to run the lease/CAS-protected,
scope-preserving `reseal-all`; a Workstation cannot authorize itself.

A metadata-only query through the installed `mcnf-secret.sh list` path found no
`resource/publisher-hmac` key. Consequently no Lighthouse reseal was requested,
`provision-resource-publisher-credential` was not run live, and the shell was not
restarted. The expected encrypted credential and systemd drop-in remain absent.
No secret value, private key, password, or recipient was printed.

## Focused verification

- Farm machine 193 (`172.20.0.90`), slot `func019-win-r6`.
- Relevant scripts passed `bash -n`.
- Publisher credential helper self-test: passed.
- Mesh secret multi-recipient reseal/rotation self-test: all passed.
- Scoped recipient/reseal self-test: all passed, including scope preservation
  and refusal to seal to an empty recipient set.
- The farm initially lacked `age`; after installing the runtime dependency, both
  crypto self-tests passed. The explicit disposable slot was then removed.

## Precise blocker

An authorized operator must first create the approved
`resource/publisher-hmac` secret under the documented mesh-secret policy; this
work did not invent or rotate it. Separately, an operator must provide valid
Windows credentials for `172.20.146.54` through the Remote Sessions prompt.
Until both exist, authenticated RDP connection/render proof remains open.
