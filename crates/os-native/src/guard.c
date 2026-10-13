/* Calls into a hosted native plugin under structured exception handling (Windows, MSVC).
 *
 * A plugin's own runtime reports errors it does not handle itself as exceptions; uncaught, one ends the whole
 * process. This frame catches them, so the host can stop the plugin and carry on (spec plugins/fruity-abi.md,
 * "Crash containment"). The plugin method is called directly from here: no Rust frame sits between the handler
 * and the plugin's code.
 *
 * spec: plugins/fruity-abi.md
 */
#include <windows.h>
#include <stdint.h>

/* Every plugin method takes at most six integer or pointer arguments. Passing six to a method that takes fewer is
   harmless in the x64 calling convention: the caller owns the argument space. */
typedef intptr_t (*os_native_method)(intptr_t, intptr_t, intptr_t, intptr_t, intptr_t, intptr_t);

#define OS_NATIVE_RUNTIME_ERROR 0x0EEDFADEUL
#define OS_NATIVE_MESSAGE 256

/* What a contained error was. Same layout as `guard::Fault`. */
typedef struct {
    unsigned long code;
    void *address;
    uintptr_t params[2];
    /* The text of the plugin runtime's error object (UTF-16, 0-terminated), empty when there is none. */
    unsigned short message[OS_NATIVE_MESSAGE];
} os_native_fault;

/* A runtime error carries its error object as the second exception parameter; the object's first field after the
   class pointer is its message, a counted UTF-16 string (spec: "Crash containment", "Reasons"). Read with care: any
   fault while reading leaves the message empty. */
static void os_native_message(const EXCEPTION_RECORD *r, os_native_fault *out)
{
    out->message[0] = 0;
    if (r->ExceptionCode != OS_NATIVE_RUNTIME_ERROR || r->NumberParameters < 2)
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
        if (len > OS_NATIVE_MESSAGE - 1)
            len = OS_NATIVE_MESSAGE - 1;
        for (i = 0; i < len; i++)
            out->message[i] = text[i];
        out->message[len] = 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        out->message[0] = 0;
    }
}

static int os_native_filter(const EXCEPTION_POINTERS *x, os_native_fault *out)
{
    const EXCEPTION_RECORD *r = x->ExceptionRecord;
    unsigned long code = r->ExceptionCode;
    /* A debugger's breakpoints and the stack overflow guard are not the plugin's errors to swallow. */
    if (code == EXCEPTION_BREAKPOINT || code == EXCEPTION_SINGLE_STEP || code == EXCEPTION_STACK_OVERFLOW)
        return EXCEPTION_CONTINUE_SEARCH;
    out->code = code;
    out->address = r->ExceptionAddress;
    out->params[0] = r->NumberParameters > 0 ? r->ExceptionInformation[0] : 0;
    out->params[1] = r->NumberParameters > 1 ? r->ExceptionInformation[1] : 0;
    os_native_message(r, out);
    return EXCEPTION_EXECUTE_HANDLER;
}

/* Calls `f`; returns 1 and its result in `ret`, or 0 and what was raised in `fault` when it raised an error. */
int os_native_guarded_call(os_native_method f, intptr_t a, intptr_t b, intptr_t c, intptr_t d, intptr_t e, intptr_t g,
                           intptr_t *ret, os_native_fault *fault)
{
    __try {
        *ret = f(a, b, c, d, e, g);
        return 1;
    } __except (os_native_filter(GetExceptionInformation(), fault)) {
        return 0;
    }
}
