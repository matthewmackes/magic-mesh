#!/usr/bin/env python3
"""Admit the Collaboration node identity from SecretStore before publication."""

from __future__ import annotations
import argparse, hashlib, json, os, re, stat, subprocess, sys, tempfile
from pathlib import Path

class Refusal(RuntimeError): pass

def regular(path: Path, label: str, owner_only: bool = False) -> bytes:
    meta = path.lstat()
    if not stat.S_ISREG(meta.st_mode) or meta.st_nlink != 1 or meta.st_mode & 0o022 or (owner_only and meta.st_mode & 0o077): raise Refusal(f"{label} is not one protected regular file")
    fd = os.open(path, os.O_RDONLY | getattr(os,"O_NOFOLLOW",0) | getattr(os,"O_CLOEXEC",0))
    try: opened=os.fstat(fd); body=os.read(fd, 1048577); after=os.fstat(fd)
    finally: os.close(fd)
    if (opened.st_dev,opened.st_ino,opened.st_size,opened.st_mtime_ns)!=(meta.st_dev,meta.st_ino,meta.st_size,meta.st_mtime_ns) or (after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns)!=(opened.st_dev,opened.st_ino,opened.st_size,opened.st_mtime_ns): raise Refusal(f"{label} changed while read")
    return body

def command(argv: list[str], input_bytes: bytes|None=None) -> bytes:
    result=subprocess.run(argv,input=input_bytes,capture_output=True,check=False)
    if result.returncode: raise Refusal(f"{argv[0]} failed with status {result.returncode}")
    return result.stdout

def main() -> int:
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--receipt",required=True,type=Path); p.add_argument("--signature",required=True,type=Path); p.add_argument("--release-public-key",required=True,type=Path)
    p.add_argument("--node",required=True); p.add_argument("--target-user",default="system:mackesd"); p.add_argument("--secret-bin",default="/usr/bin/mackesd")
    p.add_argument("--key-output",default="/var/lib/mackesd/node-signing.key",type=Path); p.add_argument("--admission-output",default="/var/lib/mackesd/collaboration-identity-admission.json",type=Path)
    a=p.parse_args(); receipt=regular(a.receipt,"receipt"); regular(a.signature,"signature"); public=regular(a.release_public_key,"release key")
    with tempfile.TemporaryDirectory(prefix="mcnf-collaboration-admit-") as td:
        keyring=Path(td)/"release.gpg"; keyring.write_bytes(command(["gpg","--batch","--dearmor"],public)); os.chmod(keyring,0o600)
        status=command(["gpgv","--status-fd","1","--keyring",str(keyring),str(a.signature),str(a.receipt)]).decode()
    signers=[]
    for line in status.splitlines():
        if line.startswith("[GNUPG:] VALIDSIG "):
            fields=line.split()
            candidate=fields[-1] if re.fullmatch(r"[0-9A-F]{40}|[0-9A-F]{64}",fields[-1]) else fields[2]
            signers.append(candidate)
    if len(signers)!=1: raise Refusal("receipt has no unique governed signature")
    try: value=json.loads(receipt.decode("ascii"))
    except (UnicodeError,json.JSONDecodeError) as exc: raise Refusal("receipt is malformed") from exc
    fields={"schema_version","kind","public_key_hex","seed_sha256","source_revision","target_node","target_user","release_signer"}
    if set(value)!=fields or value["schema_version"]!=1 or value["kind"]!="mcnf-collaboration-identity-admission" or value["release_signer"]!=signers[0] or value["target_node"]!=a.node or value["target_user"]!=a.target_user: raise Refusal("receipt is malformed, ambiguously signed, or out of scope")
    if not re.fullmatch(r"[0-9a-f]{64}",value["public_key_hex"]) or not re.fullmatch(r"[0-9a-f]{64}",value["seed_sha256"]) or not re.fullmatch(r"[0-9a-f]{40}",value["source_revision"]): raise Refusal("receipt identity fields are malformed")
    seed=command([a.secret_bin,"secret","get","collaboration/node-signing-seed"])
    if len(seed)!=32 or hashlib.sha256(seed).hexdigest()!=value["seed_sha256"]: raise Refusal("SecretStore identity does not match receipt")
    admission={key:value[key] for key in ("schema_version","kind","public_key_hex","seed_sha256","source_revision","target_node","target_user")}
    body=json.dumps(admission,sort_keys=True,separators=(",", ":")).encode("ascii")+b"\n"
    for path,data in ((a.key_output,seed),(a.admission_output,body)):
        path.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
        if path.is_symlink() or (path.exists() and (not path.is_file() or path.stat().st_nlink!=1)): raise Refusal(f"unsafe output: {path}")
        fd,temp=tempfile.mkstemp(prefix=f".{path.name}.",dir=path.parent)
        try: os.fchmod(fd,0o600 if path==a.key_output else 0o400); os.write(fd,data); os.fsync(fd); os.close(fd); fd=-1; os.replace(temp,path)
        finally:
            if fd>=0: os.close(fd)
            try: os.unlink(temp)
            except FileNotFoundError: pass
    return 0

if __name__=="__main__":
    try: raise SystemExit(main())
    except (OSError,Refusal) as exc: print(f"REFUSED[WL-FUNC-011/collaboration-identity-materializer]: {exc}",file=sys.stderr); raise SystemExit(2)
