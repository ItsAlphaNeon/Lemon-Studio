/* Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
 * and not any other part of Lemon Studio.
 */

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
/* The code of an error raised by an Object Pascal runtime (the language native plugins are built in) and not handled
   by the plugin (spec: plugins/fruity-abi.md, "Crash containment"). */
#define RUNTIME_ERROR 0x0EEDFADEu
#define MESSAGE_LEN 256

/* What a contained exception was. Same layout as `guard::Fault`. */
typedef struct {
    unsigned long code;
    /* The text of the runtime's error object (UTF-16, 0-terminated); empty when there is none. */
    unsigned short message[MESSAGE_LEN];
} os_host_fault;

/* The module this file is linked into (the program). */
static HMODULE own_module(void)
{
    static HMODULE own;
    if (!own)
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           (LPCWSTR)&own_module, &own);
    return own;
}

/* A runtime error carries its error object as the second exception parameter; the object's first field after its
   class pointer is the message, a counted UTF-16 string (spec: plugins/fruity-abi.md, "Crash containment"). Read
   with care: any fault while reading leaves the message empty. */
static void os_host_message(const EXCEPTION_RECORD *r, os_host_fault *out)
{
    out->message[0] = 0;
    if (r->ExceptionCode != RUNTIME_ERROR || r->NumberParameters < 2)
        return;
    __try {
        const unsigned char *obj = (const unsigned char *)r->ExceptionInformation[1];
        const unsigned short *text;
        int len, i;
        if (!obj)
            return;
        text = *(const unsigned short *const *)(obj + sizeof(void *));
        if (!text)
            return;
        len = *(const int *)((const unsigned char *)text - 4);
        if (len <= 0 || len > 65536)
            return;
        if (len > MESSAGE_LEN - 1)
            len = MESSAGE_LEN - 1;
        for (i = 0; i < len; i++)
            out->message[i] = text[i];
        out->message[len] = 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        out->message[0] = 0;
    }
}

static int os_host_filter(const EXCEPTION_POINTERS *ep, os_host_fault *out)
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
    out->code = code;
    os_host_message(r, out);
    return EXCEPTION_EXECUTE_HANDLER;
}

/* Calls `f(ctx)`; returns 1, or 0 with what was raised in `fault` when the call raised an exception this frame
   contains. */
int os_host_guarded_call(os_host_guarded_fn f, void *ctx, os_host_fault *fault)
{
    __try {
        f(ctx);
        return 1;
    } __except (os_host_filter(GetExceptionInformation(), fault)) {
        return 0;
    }
}
