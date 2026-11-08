/* Minimal public Win32 ABI declarations for our independent x86 launcher. */
#pragma once
typedef unsigned long DWORD;
typedef unsigned int SIZE_T;
typedef int BOOL;
typedef unsigned short WCHAR;
typedef void *HANDLE;
typedef void *LPVOID;
#define API __declspec(dllimport) __stdcall
#define NULL ((void *)0)
#define INVALID_HANDLE_VALUE ((HANDLE)-1)
typedef struct {
    DWORD cb; WCHAR *reserved, *desktop, *title;
    DWORD x, y, xsize, ysize, xchars, ychars, fill, flags;
    unsigned short show, reserved_size;
    unsigned char *reserved_bytes;
    HANDLE in, out, err;
} STARTUPINFOW;
typedef struct { HANDLE process, thread; DWORD pid, tid; } PROCESS_INFORMATION;
DWORD API GetModuleFileNameW(HANDLE, WCHAR *, DWORD);
BOOL API SetCurrentDirectoryW(const WCHAR *);
BOOL API CreateProcessW(const WCHAR *, WCHAR *, LPVOID, LPVOID, BOOL, DWORD, LPVOID, const WCHAR *, STARTUPINFOW *, PROCESS_INFORMATION *);
LPVOID API VirtualAllocEx(HANDLE, LPVOID, SIZE_T, DWORD, DWORD);
BOOL API VirtualFreeEx(HANDLE, LPVOID, SIZE_T, DWORD);
BOOL API WriteProcessMemory(HANDLE, LPVOID, const void *, SIZE_T, SIZE_T *);
HANDLE API GetModuleHandleW(const WCHAR *);
HANDLE API LoadLibraryW(const WCHAR *);
LPVOID API GetProcAddress(HANDLE, const char *);
HANDLE API CreateRemoteThread(HANDLE, LPVOID, SIZE_T, LPVOID, LPVOID, DWORD, DWORD *);
DWORD API WaitForSingleObject(HANDLE, DWORD);
BOOL API GetExitCodeThread(HANDLE, DWORD *);
BOOL API GetExitCodeProcess(HANDLE, DWORD *);
DWORD API ResumeThread(HANDLE);
BOOL API TerminateProcess(HANDLE, unsigned int);
BOOL API CloseHandle(HANDLE);
DWORD API GetLastError(void);
void API ExitProcess(unsigned int);
HANDLE API CreateFileW(const WCHAR *, DWORD, DWORD, LPVOID, DWORD, DWORD, HANDLE);
BOOL API ReadFile(HANDLE, LPVOID, DWORD, DWORD *, LPVOID);
BOOL API WriteFile(HANDLE, const void *, DWORD, DWORD *, LPVOID);
BOOL API MoveFileExW(const WCHAR *, const WCHAR *, DWORD);
