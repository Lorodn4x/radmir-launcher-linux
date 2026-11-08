/* Runs an approved Windows prerequisite without relying on wrapper exit. */
#include "win32.h"
static WCHAR folder[32768], config[32768], output[32768], data[65536];
static WCHAR command[65536];
static STARTUPINFOW si; static PROCESS_INFORMATION pi;
static void copy(WCHAR *d,const WCHAR*s){while((*d++=*s++)){} }
static void append(WCHAR*d,const WCHAR*s){while(*d)++d;copy(d,s);}
static void result(DWORD code){
    char text[48], digits[16];DWORD n=0,d=0,written;
    const char *prefix="exited ";while(*prefix)text[n++]=*prefix++;
    do{digits[d++]=(char)('0'+code%10);code/=10;}while(code);
    while(d)text[n++]=digits[--d];text[n++]='\n';
    HANDLE h=CreateFileW(output,0x40000000,1,NULL,2,0x80,NULL);
    if(h!=INVALID_HANDLE_VALUE){WriteFile(h,text,n,&written,NULL);CloseHandle(h);}
}
void entry(void){
    DWORD count=GetModuleFileNameW(NULL,folder,32760),read=0,code=0;
    if(!count||count>=32760)ExitProcess(2);
    while(count&&folder[count-1]!='\\')--count;folder[count]=0;
    copy(config,folder);append(config,(const WCHAR*)L"command.args");
    copy(output,folder);append(output,(const WCHAR*)L"command.status");
    HANDLE file=CreateFileW(config,0x80000000,1,NULL,3,0x80,NULL);
    if(file==INVALID_HANDLE_VALUE){result(2);ExitProcess(2);}
    BOOL ok=ReadFile(file,data,sizeof(data)-6,&read,NULL);CloseHandle(file);
    if(!ok||read%2){result(87);ExitProcess(87);}data[read/2]=data[read/2+1]=data[read/2+2]=0;
    if(!data[0]){result(0);ExitProcess(0);}
    WCHAR *args=data;while(*args)++args;++args;
    WCHAR *cwd=args;while(*cwd)++cwd;++cwd;
    copy(command,(const WCHAR*)L"\"");append(command,data);append(command,(const WCHAR*)L"\" ");append(command,args);
    si.cb=sizeof(si);
    if(!CreateProcessW(data,command,NULL,NULL,0,0,NULL,*cwd?cwd:NULL,&si,&pi)){code=GetLastError();result(code);ExitProcess(code);}
    CloseHandle(pi.thread);
    if(WaitForSingleObject(pi.process,0xffffffff)!=0||!GetExitCodeProcess(pi.process,&code))code=GetLastError();
    CloseHandle(pi.process);result(code);ExitProcess(code);
}
