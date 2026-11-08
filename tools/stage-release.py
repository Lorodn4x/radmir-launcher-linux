#!/usr/bin/env python3
"""Stage verified local packages, optional dependency sources and SHA256SUMS."""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import tarfile

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle-dir",type=pathlib.Path,required=True)
    parser.add_argument("--output",type=pathlib.Path,required=True)
    parser.add_argument("--sources",type=pathlib.Path,
                        help="Corresponding third-party sources (.tar.gz)")
    args=parser.parse_args()
    project=pathlib.Path(__file__).resolve().parents[1]
    version=json.loads((project/"package.json").read_text())["version"]
    candidates={}
    for directory,suffix in [("appimage",".AppImage"),("deb",".deb")]:
        paths=list((args.bundle_dir/directory).glob("*"+suffix))
        if len(paths)!=1:
            parser.error("Expected one "+directory+" artifact, found "+str(len(paths)))
        path=paths[0]
        if path.is_symlink() or "qa" in path.name.lower():
            parser.error("Refusing a symlink or QA artifact")
        candidates[directory]=path
    appimage=candidates["appimage"]
    with appimage.open("rb") as file:
        header=file.read(11)
        if header[:4]!=b"\x7fELF" or header[8:11]!=b"AI\x02":
            parser.error("Not a type-2 AppImage")
    deb=candidates["deb"]
    with deb.open("rb") as file:
        if file.read(8)!=b"!<arch>\n":
            parser.error("DEB archive signature is invalid")
    fields={}
    for field in ["Package","Version","Architecture"]:
        fields[field]=subprocess.check_output(["dpkg-deb","--field",str(deb),field],text=True).strip()
    if fields["Version"]!=version or fields["Architecture"]!="amd64" or fields["Package"]!="radmir-standalone":
        parser.error("Unexpected DEB metadata: "+json.dumps(fields))
    if args.sources:
        if args.sources.is_symlink():
            parser.error("Refusing a symlink source archive")
        with tarfile.open(args.sources,"r:gz") as archive:
            members=archive.getmembers()
            if not members or not any(m.name.endswith("/README.md") for m in members):
                parser.error("Source archive needs build and license notes")
            for member in members:
                path=pathlib.PurePosixPath(member.name)
                if path.is_absolute() or ".." in path.parts or not (member.isfile() or member.isdir()):
                    parser.error("Unsafe source archive member: "+member.name)
        candidates["sources"]=args.sources
    output=args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("Output directory must be empty")
    output.mkdir(parents=True,exist_ok=True)
    names={
        "appimage":"RADMIR-Standalone-"+version+"-x86_64.AppImage",
        "deb":"RADMIR-Standalone-"+version+"-amd64.deb",
        "sources":"RADMIR-Standalone-"+version+"-third-party-sources.tar.gz",
    }
    sums=[]
    for kind,source in candidates.items():
        destination=output/names[kind]
        shutil.copy2(source,destination)
        destination.chmod(0o755 if kind=="appimage" else 0o644)
        with destination.open("rb") as file:
            checksum=hashlib.file_digest(file,"sha256").hexdigest()
        sums.append(checksum+"  "+destination.name)
    (output/"SHA256SUMS").write_text("\n".join(sums)+"\n")
    subprocess.run(["sha256sum","--check","SHA256SUMS"],cwd=output,check=True)
    print("Prepared "+str(output)+"; no GitHub operations performed")

if __name__=="__main__":
    main()
