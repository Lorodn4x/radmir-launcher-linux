#!/usr/bin/env python3
"""Stage locally built AppImage, DEB and SHA256SUMS. Does not publish anything."""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle-dir",type=pathlib.Path,required=True)
    parser.add_argument("--output",type=pathlib.Path,required=True)
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
        if file.read(4)!=b"\x7fELF":
            parser.error("AppImage is not an ELF file")
    deb=candidates["deb"]
    with deb.open("rb") as file:
        if file.read(8)!=b"!<arch>\n":
            parser.error("DEB archive signature is invalid")
    fields={}
    for field in ["Package","Version","Architecture"]:
        fields[field]=subprocess.check_output(["dpkg-deb","--field",str(deb),field],text=True).strip()
    if fields["Version"]!=version or fields["Architecture"]!="amd64" or fields["Package"]!="radmir-standalone":
        parser.error("Unexpected DEB metadata: "+json.dumps(fields))
    output=args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("Output directory must be empty")
    output.mkdir(parents=True,exist_ok=True)
    names={
        "appimage":"RADMIR-Standalone-"+version+"-x86_64.AppImage",
        "deb":"RADMIR-Standalone-"+version+"-amd64.deb",
    }
    sums=[]
    for kind,source in candidates.items():
        destination=output/names[kind]
        shutil.copy2(source,destination)
        checksum=hashlib.file_digest(destination.open("rb"),"sha256").hexdigest()
        sums.append(checksum+"  "+destination.name)
    (output/"SHA256SUMS").write_text("\n".join(sums)+"\n")
    subprocess.run(["sha256sum","--check","SHA256SUMS"],cwd=output,check=True)
    print("Prepared "+str(output)+"; no GitHub operations performed")

if __name__=="__main__":
    main()
