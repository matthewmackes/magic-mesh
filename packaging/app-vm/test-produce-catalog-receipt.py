import json, subprocess, sys, tempfile
from pathlib import Path
TOOL=Path(__file__).with_name("produce-catalog-receipt.py")
with tempfile.TemporaryDirectory() as d:
    root=Path(d); catalog=root/"catalog.json"; out=root/"receipt.json"
    catalog.write_text(json.dumps({"schema_version":1,"remote":"curated","refs":["org.example.App/stable@sha256:"+"a"*64]})); catalog.chmod(0o444)
    assert subprocess.run([sys.executable,str(TOOL),"--catalog",str(catalog),"--source-revision","1"*40,"--source-epoch","1","--output",str(out)]).returncode==0
    value=json.loads(out.read_text()); assert value["remote"]=="curated" and out.stat().st_mode & 0o777 == 0o400
    catalog.write_text(json.dumps({"schema_version":1,"remote":"flathub","refs":["x@y"]})); catalog.chmod(0o444)
    assert subprocess.run([sys.executable,str(TOOL),"--catalog",str(catalog),"--source-revision","1"*40,"--source-epoch","1","--output",str(root/"bad")]).returncode==2
print("App VM curated catalog receipt self-test: PASS")
