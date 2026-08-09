# WL-FUNC-019 resource publisher key distribution — r9

Date: 2026-08-09

## Outcome

Basement seat 15 (172.20.0.15) reported SecretStore exit status 3 for
resource/publisher-hmac, proving that the shared publisher authority was absent
rather than merely unavailable to the shell. A fresh 32-byte random key was
then written directly to the approved mesh SecretStore through mackesd secret
put. The command reported a sealed 44-byte encoded value, and a bounded,
output-suppressed secret-get validation succeeded. No plaintext key entered the
repository, terminal output, or evidence.

This closes creation of the shared authority key. It does not claim that the
currently installed shell can consume it.

## Installed-release boundary

The subsequent controlled activation stopped at the next honest boundary:
seat 15 does not have mcnf-resource-publisher-credential.service installed.
The service restart failed with "Unit ... not found", and no unmanaged unit or
shell binary was copied onto the seat. The governed candidate contains the
helper and unit, but that candidate remains unsigned and undeployed.

Authenticated RDP activation therefore still requires a governed release that
materializes the new shared secret as the shell's host-bound systemd
credential, followed by the existing signed catalog and Windows-login path.
