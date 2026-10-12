/* Runs a call into hosted plugin code under structured exception handling (Windows, MSVC).
 *
 * An access violation or another processor exception inside a plugin, or a C++ exception it throws and does not catch
 * itself, would end the whole process. This frame catches them, so the host can stop that plugin and carry on.
 *
 * spec: plugins/plugin-formats.md ("Crash containment")
 */
#include <windows.h>

typedef void (*os_host_guarded_fn)(void *);

/* The code of a Microsoft C++ exception (`throw`). Rust panics on this target are raised the same way. */
#define CXX_EXCEPTION 0xE06D7363u

/* The module this file is linked into (the program). */
static HMODULE own_module(void)
{
    static HMODULE own;
    if (!own)
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           (LPCWSTR)&own_module, &own);
    return own;
}

static int os_host_filter(const EXCEPTION_POINTERS *ep, unsigned long *out)
{
    const EXCEPTION_RECORD *r = ep->ExceptionRecord;
    unsigned long code = r->ExceptionCode;
    /* A debugger's breakpoints and the stack overflow guard are not the plugin's errors to swallow. */
    if (code == EXCEPTION_BREAKPOINT || code == EXCEPTION_SINGLE_STEP || code == EXCEPTION_STACK_OVERFLOW)
        return EXCEPTION_CONTINUE_SEARCH;
    /* A C++ exception thrown by the program itself (a Rust panic) unwinds as it would without this frame. On 64-bit
       Windows its fourth parameter is the image base of the module that threw it. */
    if (code == CXX_EXCEPTION && r->NumberParameters >= 4 && (HMODULE)r->ExceptionInformation[3] == own_module())
        return EXCEPTION_CONTINUE_SEARCH;
    *out = code;
    return EXCEPTION_EXECUTE_HANDLER;
}

/* Calls `f(ctx)`; returns 1, or 0 with the exception code in `code` when the call raised one this frame contains. */
int os_host_guarded_call(os_host_guarded_fn f, void *ctx, unsigned long *code)
{
    __try {
        f(ctx);
        return 1;
    } __except (os_host_filter(GetExceptionInformation(), code)) {
        return 0;
    }
}
