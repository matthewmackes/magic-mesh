-- SEC-006 — retain only the requester-owned Nebula public key at the CA.
-- The CA needs this across epoch rotation so it can re-sign with `-in-pub`
-- without ever generating, receiving, or replicating the peer private key.
ALTER TABLE nebula_peer_certs ADD COLUMN public_key_pem TEXT;
