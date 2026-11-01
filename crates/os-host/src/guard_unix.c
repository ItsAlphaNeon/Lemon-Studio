/* Copyright (C) 2026 ItsAlphaNeon
 * SPDX-License-Identifier: GPL-3.0-or-later
 * This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
 */

/* Runs a call into hosted plugin code under a signal guard (macOS and Linux): the counterpart of `guard.c`.
 *
 * A memory error, an invalid instruction or an arithmetic trap inside a plugin raises SIGSEGV, SIGBUS, SIGILL or
 * SIGFPE, whose default action ends the whole process. While a guarded call runs on a thread, this file's handler
 * jumps from the signal back to the start of that call (sigsetjmp / siglongjmp through a thread-local frame), and the
 * call returns 0, so the host can stop that plugin and carry on.
 *
 * Unlike structured exception handling, the jump does not unwind: everything between the guarded call and the fault
 * is abandoned as it was. Plugin code is meant to be abandoned (the plugin is never called again), but Rust frames of
 * the host's own glue are abandoned too, without their destructors: what they own leaks, and a lock one of them holds
 * stays held. See `guard.rs` for what the host does about that.
 *
 * A signal that is not a fault inside a guarded call (one on a thread without a guarded call, or one another process
 * or `raise` sent) goes to the handler that was installed before this one, or gets the default action.
 *
 * spec: plugins/plugin-formats.md ("Crash containment")
 */
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>

typedef void (*os_host_guarded_fn)(void *);

#define MESSAGE_LEN 256

/* What a contained crash was. Same layout as `guard::Fault`. */
typedef struct {
    unsigned int code;
    /* Always empty here (`guard.c` fills it from a plugin runtime's error object). */
    unsigned short message[MESSAGE_LEN];
} os_host_fault;

/* The Windows exception codes the host reports crashes by (`guard::describe`). */
#define ACCESS_VIOLATION 0xC0000005u
#define ILLEGAL_INSTRUCTION 0xC000001Du
#define INTEGER_DIVIDE_BY_ZERO 0xC0000094u
#define FLOAT_ERROR 0xC0000090u

/* A guarded call running on this thread. Frames of nested calls are chained, innermost first. */
typedef struct os_host_frame {
    sigjmp_buf env;
    struct os_host_frame *prev;
    /* The signal that ended the call, and what the system said about it. */
    volatile int sig;
    volatile int sig_code;
} os_host_frame;

#if defined(__GNUC__) || defined(__clang__)
/* Initial-exec: read in a signal handler, where the general model's lazy allocation is not allowed. */
#define TLS __thread __attribute__((tls_model("initial-exec")))
#else
#define TLS __thread
#endif

static TLS os_host_frame *current;
/* This thread has an alternate signal stack (its own, or one from `ensure_stack`). */
static TLS int stack_ready;

static const int SIGNALS[] = {SIGSEGV, SIGBUS, SIGILL, SIGFPE};
#define SIGNAL_COUNT (sizeof SIGNALS / sizeof SIGNALS[0])
/* What each signal did before this file's handler was installed. */
static struct sigaction previous[SIGNAL_COUNT];
static pthread_once_t installed = PTHREAD_ONCE_INIT;
static pthread_key_t stack_key;

/* The signal was raised by the processor for the code that was running (not sent by `kill`, `raise` or the like). */
static int from_fault(const siginfo_t *info)
{
    if (!info)
        return 0;
#if defined(__APPLE__)
    /* SI_USER and the other codes of sent signals are 0x10001 and up. */
    return info->si_code > 0 && info->si_code < 0x10000;
#else
    /* SI_USER is 0, SI_QUEUE, SI_TKILL and the others are negative. */
    return info->si_code > 0;
#endif
}

/* Hands `sig` on to what handled it before this file's handler, or to the default action. */
static void chain(int sig, siginfo_t *info, void *context)
{
    for (unsigned i = 0; i < SIGNAL_COUNT; i++) {
        if (SIGNALS[i] != sig)
            continue;
        const struct sigaction *old = &previous[i];
        if (old->sa_flags & SA_SIGINFO) {
            if (old->sa_sigaction) {
                old->sa_sigaction(sig, info, context);
                return;
            }
        } else if (old->sa_handler != SIG_DFL && old->sa_handler != SIG_IGN) {
            old->sa_handler(sig);
            return;
        }
        break;
    }
    /* The default action. A fault happens again as soon as the handler returns and then ends the process the usual
       way (with a core dump where those are on); a sent signal is sent again, and arrives once the handler returns. */
    struct sigaction dfl;
    memset(&dfl, 0, sizeof dfl);
    dfl.sa_handler = SIG_DFL;
    sigemptyset(&dfl.sa_mask);
    sigaction(sig, &dfl, NULL);
    if (!from_fault(info))
        raise(sig);
}

static void on_signal(int sig, siginfo_t *info, void *context)
{
    os_host_frame *fr = current;
    if (fr && from_fault(info)) {
        fr->sig = sig;
        fr->sig_code = info->si_code;
        current = fr->prev;
        /* Restores the signal mask of the guarded call's start, which unblocks `sig` again. */
        siglongjmp(fr->env, 1);
    }
    chain(sig, info, context);
}

/* Frees the alternate signal stack `ensure_stack` gave a thread, when the thread ends. */
static void free_stack(void *mem)
{
    stack_t cur;
    if (sigaltstack(NULL, &cur) == 0 && cur.ss_sp == mem && !(cur.ss_flags & SS_ONSTACK)) {
        stack_t off;
        memset(&off, 0, sizeof off);
        off.ss_flags = SS_DISABLE;
        sigaltstack(&off, NULL);
    }
    free(mem);
}

static void install(void)
{
    pthread_key_create(&stack_key, free_stack);
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = on_signal;
    sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&sa.sa_mask);
    for (unsigned i = 0; i < SIGNAL_COUNT; i++)
        sigaction(SIGNALS[i], &sa, &previous[i]);
}

/* Gives this thread an alternate signal stack when it has none, so that a fault from a stack overflow can still be
   handled. Threads the Rust runtime starts already have one; threads a system audio API starts may not. */
static void ensure_stack(void)
{
    if (stack_ready)
        return;
    stack_ready = 1;
    stack_t cur;
    if (sigaltstack(NULL, &cur) != 0 || !(cur.ss_flags & SS_DISABLE))
        return;
    size_t size = 64 * 1024;
    if (size < (size_t)SIGSTKSZ)
        size = (size_t)SIGSTKSZ;
    void *mem = malloc(size);
    if (!mem)
        return;
    stack_t ss;
    memset(&ss, 0, sizeof ss);
    ss.ss_sp = mem;
    ss.ss_size = size;
    if (sigaltstack(&ss, NULL) != 0) {
        free(mem);
        return;
    }
    pthread_setspecific(stack_key, mem);
}

/* The exception code `guard.c` would report for the same crash. */
static unsigned int code_of(int sig, int sig_code)
{
    switch (sig) {
    case SIGILL:
        return ILLEGAL_INSTRUCTION;
    case SIGFPE:
        return sig_code == FPE_INTDIV || sig_code == FPE_FLTDIV ? INTEGER_DIVIDE_BY_ZERO : FLOAT_ERROR;
    default:
        return ACCESS_VIOLATION;
    }
}

/* Calls `f(ctx)`. Returns 1 when it returned, or 0 with the crash in `fault` when a fault ended it.
 *
 * Between this frame and the fault, frames are abandoned without running any cleanup: `f` must not let a Rust panic
 * or a C++ exception unwind into this frame either (`guard.rs` catches panics before they get here). */
int os_host_guarded_call(os_host_guarded_fn f, void *ctx, os_host_fault *fault)
{
    pthread_once(&installed, install);
    ensure_stack();
    os_host_frame fr;
    fr.prev = current;
    fr.sig = 0;
    fr.sig_code = 0;
    if (sigsetjmp(fr.env, 1) == 0) {
        current = &fr;
        f(ctx);
        current = fr.prev;
        return 1;
    }
    /* The handler already took this frame off the chain. */
    fault->code = code_of(fr.sig, fr.sig_code);
    fault->message[0] = 0;
    return 0;
}
