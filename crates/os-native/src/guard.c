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

static int os_native_filter(unsigned long code, unsigned long *out)
{
    /* A debugger's breakpoints and the stack overflow guard are not the plugin's errors to swallow. */
    if (code == EXCEPTION_BREAKPOINT || code == EXCEPTION_SINGLE_STEP || code == EXCEPTION_STACK_OVERFLOW)
        return EXCEPTION_CONTINUE_SEARCH;
    *out = code;
    return EXCEPTION_EXECUTE_HANDLER;
}

/* Calls `f`; returns 1 and its result in `ret`, or 0 and the exception code in `code` when it raised one. */
int os_native_guarded_call(os_native_method f, intptr_t a, intptr_t b, intptr_t c, intptr_t d, intptr_t e, intptr_t g,
                           intptr_t *ret, unsigned long *code)
{
    __try {
        *ret = f(a, b, c, d, e, g);
        return 1;
    } __except (os_native_filter(GetExceptionCode(), code)) {
        return 0;
    }
}
