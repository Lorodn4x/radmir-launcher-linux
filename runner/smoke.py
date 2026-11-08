#!/usr/bin/env python3
"""Exercise runner under Wine with synthetic game/DLL fixtures, never real game.
Usage: python3 runner/smoke.py /path/to/wine
Checks actual DLL order before main, target exit code, failed DLL load.
"""
import os
import pathlib
import subprocess
import sys
import tempfile
from build import build, ROOT

with tempfile.TemporaryDirectory(prefix='radmir-runner-test-') as temp:
    root=pathlib.Path(temp)
    game=root/'Game with spaces Игpa'
    sidecar=game/'.radmir'
    sidecar.mkdir(parents=True)
    (sidecar/'runner.id').write_text('11111111-2222-3333-4444-555555555555')
    (sidecar/'runner.exe').write_bytes((ROOT/'runner.exe').read_bytes())
    (sidecar/'runner.args').write_bytes('-c -n Test_Player -h 127.0.0.1 -p 7777 -t 12345 -g'.encode('utf-16-le'))
    fixture='''#include "win32.h"
static DWORD read_marker(void) {
 HANDLE f=CreateFileW((WCHAR*)L"order",0x80000000,1,NULL,3,0x80,NULL);
 DWORD value=0,n=0; if(f!=INVALID_HANDLE_VALUE) { ReadFile(f,&value,4,&n,NULL); CloseHandle(f); } return value;
}
'''
    for index,name in [(1,'clientside'),(2,'samp')]:
        source=root/f'{name}.c'
        source.write_text(fixture+f'''
BOOL __stdcall dll_entry(HANDLE module, DWORD reason, LPVOID reserved) {{
 (void)module; (void)reserved;
 if(reason==1) {{
   if(read_marker()!={index-1}) return 0;
   DWORD value={index},n;
   HANDLE f=CreateFileW((WCHAR*)L"order",0x40000000,1,NULL,2,0x80,NULL);
   if(f==INVALID_HANDLE_VALUE) return 0;
   WriteFile(f,&value,4,&n,NULL); CloseHandle(f);
 }}
 return 1;
}}
''')
        build(source,game/f'{name}.dll',True)
    source=root/'game.c'
    source.write_text(fixture+'void entry(void) { ExitProcess(read_marker()==2 ? 37 : 99); }\n')
    build(source,game/'gta_sa.exe')
    env=os.environ.copy()
    env.update(WINEPREFIX=str(root/'prefix'),WINEDEBUG='-all',WINEDLLOVERRIDES='mscoree,mshtml=')
    result=subprocess.run([sys.argv[1],str(sidecar/'runner.exe')],env=env,capture_output=True,timeout=90)
    assert (sidecar/'runner.status').exists(), (result.returncode, result.stdout.decode(errors='replace')[-1000:], result.stderr.decode(errors='replace')[-2000:])
    status=(sidecar/'runner.status').read_text().strip()
    assert status=='exited 37 11111111-2222-3333-4444-555555555555', (result.returncode,status,result.stderr.decode(errors='replace')[-1000:])
    assert result.returncode==37, result.returncode
    print('PASS: clientside → samp → main; Unicode/spaced path; exit 37 propagated')
    (game/'order').unlink()
    (game/'samp.dll').unlink()
    result=subprocess.run([sys.argv[1],str(sidecar/'runner.exe')],env=env,capture_output=True,timeout=90)
    status=(sidecar/'runner.status').read_text().strip()
    assert status.startswith('failed-samp '), status
    assert int.from_bytes((game/'order').read_bytes(),'little')==1
    print('PASS: missing samp DLL fails before main; target terminated')
    subprocess.run([str(pathlib.Path(sys.argv[1]).with_name('wineserver')),'-k'],env=env,timeout=10,check=False)
