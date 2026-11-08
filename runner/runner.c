/* RADMIR compatibility runner. Independent implementation of the observed
 * launch contract; compiled as PE32 and run in the game's Wine prefix.
 * The helper lives at <game>/.radmir/runner.exe; all sidecars stay there.
 */
#include "win32.h"

static WCHAR root[32768], exe[32768], dll[32768], args[16384];
static WCHAR status_path[32768], temp_path[32768], args_path[32768];
static STARTUPINFOW startup;
static PROCESS_INFORMATION child;
static DWORD last_error;
static char run_id[40];

static unsigned int length(const WCHAR *s) { unsigned int n=0; while(s[n]) ++n; return n; }
static void copy(WCHAR *to, const WCHAR *from) { while ((*to++ = *from++)) {} }
static void append(WCHAR *to, const WCHAR *s) { copy(to+length(to), s); }
static void parent(WCHAR *s) { unsigned int n=length(s); while(n && s[n-1]!='\\') --n; if(n) s[n-1]=0; }

static void status(const char *stage, DWORD code) {
    char data[96], digits[16]; unsigned int n=0, d=0; DWORD written;
    while (*stage) data[n++]=*stage++;
    data[n++]=' ';
    do { digits[d++]=(char)('0'+code%10); code/=10; } while(code);
    while(d) data[n++]=digits[--d];
    data[n++]=' ';
    for(unsigned int i=0;run_id[i];++i)data[n++]=run_id[i];
    data[n++]='\n';
    HANDLE file=CreateFileW(temp_path,0x40000000,1,NULL,2,0x80,NULL);
    if(file!=INVALID_HANDLE_VALUE) {
        WriteFile(file,data,n,&written,NULL); CloseHandle(file);
        MoveFileExW(temp_path,status_path,1|8);
    }
}

static void fail(const char *stage) {
    DWORD error=last_error ? last_error : GetLastError();
    if(!error) error=1;
    if(child.process) { TerminateProcess(child.process,error); CloseHandle(child.process); }
    if(child.thread) CloseHandle(child.thread);
    status(stage,error); ExitProcess(error);
}

static BOOL inject(const WCHAR *path) {
    SIZE_T size=(length(path)+1)*2, written=0;
    LPVOID memory=VirtualAllocEx(child.process,NULL,size,0x3000,4);
    if(!memory) { last_error=GetLastError(); return 0; }
    BOOL ok=WriteProcessMemory(child.process,memory,path,size,&written);
    HANDLE thread=NULL;
    if(ok && written==size) {
        LPVOID load=GetProcAddress(GetModuleHandleW((const WCHAR *)L"kernel32.dll"),"LoadLibraryW");
        if(load) thread=CreateRemoteThread(child.process,NULL,0,load,memory,0,NULL);
    }
    if(!thread) { last_error=GetLastError(); ok=0; }
    else {
        DWORD result=WaitForSingleObject(thread,60000), module=0;
        ok=result==0 && GetExitCodeThread(thread,&module) && module!=0;
        if(!ok) last_error=result==258 ? 1460 : 1114;
        CloseHandle(thread);
        /* On timeout the loader may still use this allocation: kill target
         * before returning, never free a live remote thread's argument. */
        if(result!=0) TerminateProcess(child.process,last_error);
    }
    VirtualFreeEx(child.process,memory,0,0x8000);
    return ok;
}

void entry(void) {
    DWORD count=GetModuleFileNameW(NULL,root,32000), read=0, code=0;
    if(!count || count>=32000) ExitProcess(2);
    parent(root);
    copy(status_path,root); append(status_path,(const WCHAR *)L"\\runner.status");
    copy(temp_path,root); append(temp_path,(const WCHAR *)L"\\runner.status.tmp");
    copy(args_path,root); append(args_path,(const WCHAR *)L"\\runner.args");
    copy(args_path,root); append(args_path,(const WCHAR *)L"\\runner.id");
    HANDLE identity=CreateFileW(args_path,0x80000000,1,NULL,3,0x80,NULL);
    if(identity==INVALID_HANDLE_VALUE)ExitProcess(87);
    DWORD id_read=0; BOOL id_ok=ReadFile(identity,run_id,36,&id_read,NULL);CloseHandle(identity);
    if(!id_ok||id_read!=36)ExitProcess(87);run_id[36]=0;
    copy(args_path,root);append(args_path,(const WCHAR *)L"\\runner.args");
    status("starting",0);
    HANDLE config=CreateFileW(args_path,0x80000000,1,NULL,3,0x80,NULL);
    if(config==INVALID_HANDLE_VALUE) fail("failed-config");
    BOOL ok=ReadFile(config,args,sizeof(args)-2,&read,NULL); CloseHandle(config);
    if(!ok || !read || read%2 || read>=sizeof(args)-2) { last_error=87; fail("failed-config"); }
    args[read/2]=0;
    parent(root);
    if(!SetCurrentDirectoryW(root)) fail("failed-directory");
    copy(exe,root); append(exe,(const WCHAR *)L"\\gta_sa.exe");
    startup.cb=sizeof(startup);
    /* Keep the argument-only command line used by the original launcher. */
    if(!CreateProcessW(exe,args,NULL,NULL,0,4,NULL,root,&startup,&child)) fail("failed-create");
    status("clientside",child.pid);
    copy(dll,root); append(dll,(const WCHAR *)L"\\clientside.dll");
    if(!inject(dll)) fail("failed-clientside");
    status("samp",child.pid);
    copy(dll,root); append(dll,(const WCHAR *)L"\\samp.dll");
    if(!inject(dll)) fail("failed-samp");
    if(ResumeThread(child.thread)==0xffffffff) fail("failed-resume");
    CloseHandle(child.thread); child.thread=NULL;
    for(;;) {
        status("running",child.pid);
        DWORD wait=WaitForSingleObject(child.process,1000);
        if(wait==0) break;
        if(wait!=258) fail("failed-wait");
    }
    if(!GetExitCodeProcess(child.process,&code)) fail("failed-exit");
    CloseHandle(child.process); child.process=NULL;
    status("exited",code); ExitProcess(code);
}
