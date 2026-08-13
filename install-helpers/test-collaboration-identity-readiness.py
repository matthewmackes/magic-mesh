#!/usr/bin/env python3
"""Hostile producer/materializer integration for WL-FUNC-011."""
import json, os, subprocess, tempfile
from pathlib import Path

REPO=Path(__file__).resolve().parents[1]
PRODUCER=REPO/"install-helpers/produce-collaboration-identity-receipt.py"
MATERIALIZER=REPO/"install-helpers/materialize-collaboration-identity.py"

def run(argv, env, ok=True):
    result=subprocess.run([str(x) for x in argv],env=env,capture_output=True,text=True,check=False)
    if (result.returncode==0)!=ok: raise AssertionError(f"unexpected status {result.returncode}: {result.stderr}")
    return result

def main():
    with tempfile.TemporaryDirectory() as td:
        root=Path(td); env=dict(os.environ,GNUPGHOME=str(root/"gnupg")); (root/"gnupg").mkdir(mode=0o700)
        run(["gpg","--batch","--passphrase","","--quick-generate-key","Collaboration fixture <fixture.invalid>","ed25519","sign","0"],env)
        listing=run(["gpg","--batch","--with-colons","--fingerprint","--list-secret-keys"],env).stdout
        fingerprint=next(row.split(":")[9] for row in listing.splitlines() if row.startswith("fpr:"))
        public=root/"release.asc"; public.write_text(run(["gpg","--batch","--armor","--export",fingerprint],env).stdout); public.chmod(0o400)
        seed=root/"seed"; seed.write_bytes(bytes(range(32))); seed.chmod(0o400)
        receipt=root/"receipt.json"; revision="a"*40; public_hex="b"*64
        args=[PRODUCER,"--secret-store-export",seed,"--public-key-hex",public_hex,"--source-revision",revision,"--target-node","peer:seat-a","--output-receipt",receipt,"--release-public-key",public,"--release-key-id",fingerprint]
        run(args,env)
        fake=root/"mackesd"; fake.write_text(f"#!/bin/sh\ndd if='{seed}' bs=32 count=1 status=none\n"); fake.chmod(0o700)
        key=root/"installed.key"; admission=root/"admission.json"
        material=[MATERIALIZER,"--receipt",receipt,"--signature",str(receipt)+".asc","--release-public-key",public,"--node","peer:seat-a","--secret-bin",fake,"--key-output",key,"--admission-output",admission]
        run(material,env); assert key.read_bytes()==seed.read_bytes(); assert json.loads(admission.read_text())["source_revision"]==revision
        admission.unlink(); key.unlink(); wrong_node=material.copy(); wrong_node[8]="peer:seat-b"; run(wrong_node,env,ok=False)
        bad=root/"bad-mackesd"; bad.write_text("#!/bin/sh\nhead -c 32 /dev/zero\n"); bad.chmod(0o700)
        wrong_secret=material.copy(); wrong_secret[10]=bad; run(wrong_secret,env,ok=False)
        tampered=root/"tampered.json"; tampered.write_bytes(receipt.read_bytes().replace(b"peer:seat-a",b"peer:seat-b")); tampered.chmod(0o400)
        run([MATERIALIZER,"--receipt",tampered,"--signature",str(receipt)+".asc","--release-public-key",public,"--node","peer:seat-b","--secret-bin",fake,"--key-output",key,"--admission-output",admission],env,ok=False)
    print("test-collaboration-identity-readiness: hostile integration passed")

if __name__=="__main__": main()
