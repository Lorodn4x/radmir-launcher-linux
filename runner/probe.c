/* PE32 graphics probe uses the actual Wine/DirectDraw/D3D9 implementation. */
#include "win32.h"
typedef struct {unsigned long a;unsigned short b,c;unsigned char d[8];} GUID;
typedef struct {DWORD width,height,format,count,multisample,quality,swap;HANDLE window;BOOL windowed,depth;DWORD depth_format,flags,refresh,interval;} PRESENT;
typedef DWORD (__stdcall *DDCREATE)(const GUID*,LPVOID*,const GUID*,LPVOID);
typedef LPVOID (__stdcall *D3DCREATE)(DWORD);
typedef DWORD (__stdcall *RELEASE)(LPVOID);
typedef DWORD (__stdcall *COUNT)(LPVOID);
typedef DWORD (__stdcall *DEVICE)(LPVOID,DWORD,DWORD,HANDLE,DWORD,PRESENT*,LPVOID*);
typedef HANDLE (__stdcall *DESKTOP)(void);
static WCHAR path[32768];
static void report(const char *stage,DWORD code){
    char text[128],digits[16];DWORD n=0,d=0,written;while(*stage)text[n++]=*stage++;
    text[n++]=' ';do{digits[d++]=(char)('0'+code%10);code/=10;}while(code);while(d)text[n++]=digits[--d];text[n++]='\n';
    HANDLE f=CreateFileW(path,0x40000000,1,NULL,2,0x80,NULL);if(f!=INVALID_HANDLE_VALUE){WriteFile(f,text,n,&written,NULL);CloseHandle(f);}
}
static void fail(const char *stage,DWORD code){report(stage,code?code:1);ExitProcess(code?code:1);}
void entry(void){
    DWORD length=GetModuleFileNameW(NULL,path,32740);if(!length||length>=32740)ExitProcess(2);
    while(length&&path[length-1]!='\\')--length;
    const WCHAR *suffix=(const WCHAR*)L"probe.status";while((* (path+length++)=*suffix++)){}
    HANDLE module=LoadLibraryW((const WCHAR*)L"ddraw.dll");if(!module)fail("failed-ddraw-load",GetLastError());
    DDCREATE create=(DDCREATE)GetProcAddress(module,"DirectDrawCreateEx");if(!create)fail("failed-ddraw-api",GetLastError());
    GUID iid={0x15e65ec0,0x3b9c,0x11d2,{0xb9,0x2f,0,0x60,0x97,0x97,0xea,0x5b}};LPVOID object=NULL;
    DWORD code=create(NULL,&object,&iid,NULL);if(code||!object)fail("failed-directdraw",code);
    ((RELEASE)(*(LPVOID**)object)[2])(object);
    module=LoadLibraryW((const WCHAR*)L"d3d9.dll");if(!module)fail("failed-d3d9-load",GetLastError());
    D3DCREATE d3d=(D3DCREATE)GetProcAddress(module,"Direct3DCreate9");if(!d3d)fail("failed-d3d9-api",GetLastError());
    object=d3d(32);if(!object)fail("failed-d3d9",1);
    if(!((COUNT)(*(LPVOID**)object)[4])(object))fail("failed-adapter",1);
    HANDLE user=LoadLibraryW((const WCHAR*)L"user32.dll");DESKTOP desktop=(DESKTOP)GetProcAddress(user,"GetDesktopWindow");if(!desktop)fail("failed-window",1);
    PRESENT pp={0};pp.width=64;pp.height=64;pp.count=1;pp.swap=1;pp.window=desktop();pp.windowed=1;
    LPVOID device=NULL;code=((DEVICE)(*(LPVOID**)object)[16])(object,0,1,pp.window,0x20,&pp,&device);
    if(code||!device)fail("failed-device",code);
    ((RELEASE)(*(LPVOID**)device)[2])(device);((RELEASE)(*(LPVOID**)object)[2])(object);
    report("ready",0);ExitProcess(0);
}
