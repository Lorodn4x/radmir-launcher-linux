#!/usr/bin/env python3
"""Reproducible PE32 build using installed clang + GNU ld, no MinGW SDK.

Only the standard Win32 import table is generated; machine code comes from
runner.c. Run this before cargo build; the result is embedded by Rust.
"""
import pathlib
import re
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent
IMPORTS = {
    'GetModuleFileNameW':12, 'SetCurrentDirectoryW':4, 'CreateProcessW':40,
    'VirtualAllocEx':20, 'VirtualFreeEx':16, 'WriteProcessMemory':20,
    'GetModuleHandleW':4, 'LoadLibraryW':4, 'GetProcAddress':8, 'CreateRemoteThread':28,
    'WaitForSingleObject':8, 'GetExitCodeThread':8, 'GetExitCodeProcess':8,
    'ResumeThread':4, 'TerminateProcess':8, 'CloseHandle':4,
    'GetLastError':0, 'ExitProcess':4, 'CreateFileW':28, 'ReadFile':20,
    'WriteFile':20, 'MoveFileExW':12,
}

def build(source=None, output=None, dll=False):
    source = source or ROOT/'runner.c'
    output = output or ROOT/'runner.exe'
    with tempfile.TemporaryDirectory(prefix='radmir-pe-') as tmp:
        tmp = pathlib.Path(tmp)
        lines = ['.section .idata$2', '.rva lookup', '.long 0,0',
                 '.rva dllname', '.rva addresses', '.section .idata$3', '.long 0,0,0,0,0',
                 '.section .idata$4', 'lookup:']
        lines += [f'.rva name_{name}' for name in IMPORTS] + ['.long 0', '.section .idata$5', 'addresses:']
        for name, size in IMPORTS.items():
            lines += [f'.globl "__imp__{name}@{size}"', f'"__imp__{name}@{size}":', f'.rva name_{name}']
        lines += ['.long 0', '.section .idata$6']
        for name in IMPORTS:
            lines += ['.balign 2', f'name_{name}:', '.short 0', f'.asciz "{name}"']
        lines += ['dllname:', '.asciz "KERNEL32.dll"']
        (tmp/'imports.s').write_text('\n'.join(lines)+'\n')
        subprocess.run(['clang','--target=i686-w64-windows-gnu','-Os','-ffreestanding',
                        '-fno-stack-protector','-fno-builtin','-Wall','-Wextra','-Werror',
                        '-c',str(source),'-I',str(ROOT),'-o',str(tmp/'runner.o')],check=True)
        subprocess.run(['clang','--target=i686-w64-windows-gnu','-c',str(tmp/'imports.s'),'-o',str(tmp/'imports.o')],check=True)
        subsystem = 'windows' if source.name in ('runner.c','command.c','probe.c') else 'console'
        subprocess.run(['ld','-mi386pe','--entry','_dll_entry@12' if dll else '_entry','--subsystem',subsystem,
                        '--enable-reloc-section','--dynamicbase','--nxcompat','--no-insert-timestamp',
                        *(['--dll'] if dll else []),
                        '-s',str(tmp/'runner.o'),str(tmp/'imports.o'),'-o',str(output)],check=True)
        data=output.read_bytes()
        assert data[:2]==b'MZ'
        dump=subprocess.check_output(['objdump','-p',str(output)],text=True)
        assert 'pei-i386' in dump and 'KERNEL32.dll' in dump
        assert all(re.search(r'\b'+name+r'\b',dump) for name in IMPORTS)
        print(f'Built PE32 helper: {len(data)} bytes; {len(IMPORTS)} Win32 imports')

if __name__=='__main__':
    build()
    build(ROOT/'command.c',ROOT/'command.exe')
    build(ROOT/'probe.c',ROOT/'probe.exe')
