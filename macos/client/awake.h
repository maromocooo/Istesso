// Keeping a fast processor core awake while the person works in the window (--wake awake-spin,
// awake-tick; docs/adr/0018-own-client.md). A measuring switch, off unless named, and not
// adopted: without it nothing here runs and no thread is made.
//
// One thread in the highest interactive class either never lets go of its core (spin) or wakes
// a thousand times a second (tick), so that the processor's fast cores neither go to sleep nor
// slow down between two frames and two keys. Here: the rule for when, and the thread. What the
// rule reads about the window comes from the application. Tested by awake-test.c, without a
// window. Included once, by one file.
#ifndef ISTESSO_AWAKE_H
#define ISTESSO_AWAKE_H

#include <mach/mach_time.h>
#include <pthread.h>
#include <pthread/qos.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>

#define AWAKE_OFF 0
#define AWAKE_SPIN 1  // one thread never lets go of its core
#define AWAKE_TICK 2  // one thread wakes every AWAKE_TICK_NS and does nothing

// How long after the latest input the core is kept awake. Between the keys of a word lie tenths
// of a second, between words and sentences up to about two seconds; a silence longer than that
// is the person reading or thinking, and the first key after it finds the core as it would be
// without the switch. Three seconds bridge the pauses inside typing with room to spare, and bound
// what is spent after the last key to three seconds of one core.
#define AWAKE_AFTER_INPUT_NS 3000000000ULL

// The interval of the cheaper form: a thousand wake-ups a second.
#define AWAKE_TICK_NS 1000000ULL

// Whether the core is kept awake at `now_ns`. `whole_session`: from the program's start to its
// end, whatever the window and the person do (for measuring). Otherwise only while the window is
// the one in front (`in_front`: the application is the active one and the window takes the keys)
// and input was passed on less than AWAKE_AFTER_INPUT_NS ago (`last_input_ns`, 0 for never).
// Times are of one clock; an input noted just after `now_ns` was read counts as now.
static inline bool awake_holds(bool whole_session, bool in_front, uint64_t last_input_ns, uint64_t now_ns) {
    if (whole_session) {
        return true;
    }
    if (!in_front || last_input_ns == 0) {
        return false;
    }
    return now_ns <= last_input_ns || now_ns - last_input_ns < AWAKE_AFTER_INPUT_NS;
}

// What the spinning thread does between two looks at the clock: the processor's own instruction
// for "waiting in a loop", which keeps the core busy without work for it to do.
static inline void awake_pause(void) {
#if defined(__arm64__) || defined(__aarch64__)
    __asm__ volatile("yield");
#elif defined(__x86_64__) || defined(__i386__)
    __builtin_ia32_pause();
#endif
}

// ---- The thread ----

static int awake_form = AWAKE_OFF;        // --wake awake-spin, awake-tick
static bool awake_whole_session = false;  // --wake awake-session
// Where the thread says what it does (the frame log), or NULL.
static FILE *awake_log = NULL;
// When input was last handed to the input's thread (0: never), and whether the window is the
// one in front.
static _Atomic uint64_t awake_last_input_ns = 0;
static _Atomic bool awake_in_front = false;
// The thread rests while the rule does not hold. Whoever changes what the rule reads wakes it,
// if it is resting.
static _Atomic bool awake_resting = false, awake_over = false;
static pthread_mutex_t awake_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t awake_changed = PTHREAD_COND_INITIALIZER;
static pthread_t awake_runs;
static bool awake_started = false;
// What ran. The thread writes them when a stretch ends; read once the thread has ended.
static uint64_t awake_on_ns = 0, awake_stretches = 0, awake_turns = 0, awake_cpu_ns = 0;

// The clock of the frame log.
static inline uint64_t awake_clock(void) {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

static inline const char *awake_word(void) {
    return awake_form == AWAKE_SPIN ? "spin" : "tick";
}

static inline bool awake_now(void) {
    return awake_holds(awake_whole_session, atomic_load(&awake_in_front), atomic_load(&awake_last_input_ns), awake_clock());
}

// What the rule reads has changed. Costs one look at a flag while the thread is not resting.
static inline void awake_stir(void) {
    if (atomic_load(&awake_resting)) {
        pthread_mutex_lock(&awake_lock);
        pthread_cond_signal(&awake_changed);
        pthread_mutex_unlock(&awake_lock);
    }
}

// Input was handed on. From any thread; only with the switch.
static inline void awake_input(void) {
    atomic_store(&awake_last_input_ns, awake_clock());
    awake_stir();
}

// The window became the one in front, or stopped being it. Returns whether that is a change.
static inline bool awake_set_in_front(bool in_front) {
    if (atomic_exchange(&awake_in_front, in_front) == in_front) {
        return false;
    }
    awake_stir();
    return true;
}

static void *awake_thread(void *unused) {
    (void)unused;
    pthread_setname_np("istesso awake");
    // Work a person is waiting for: the system puts that on the fast cores when it can.
    int refused = pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    if (awake_log != NULL) {
        // `interactive`: 1 in effect, 2 asked for and not in effect.
        fprintf(awake_log, "{\"kind\":\"wake_effect\",\"t_ns\":%llu,\"awake\":\"%s\",\"when\":\"%s\",\"interactive\":%d}\n",
                (unsigned long long)awake_clock(), awake_word(), awake_whole_session ? "session" : "input_in_front",
                refused == 0 ? 1 : 2);
    }
    mach_timebase_info_data_t base;
    mach_timebase_info(&base);
    for (;;) {
        // "Resting" is said before the rule is looked at, and both under the lock: a change
        // that comes after the look finds the thread resting and wakes it.
        pthread_mutex_lock(&awake_lock);
        atomic_store(&awake_resting, true);
        while (!atomic_load(&awake_over) && !awake_now()) {
            pthread_cond_wait(&awake_changed, &awake_lock);
        }
        atomic_store(&awake_resting, false);
        pthread_mutex_unlock(&awake_lock);
        if (atomic_load(&awake_over)) {
            break;
        }
        uint64_t began = awake_clock(), turns = 0;
        if (awake_log != NULL) {
            fprintf(awake_log, "{\"kind\":\"awake\",\"t_ns\":%llu,\"on\":1}\n", (unsigned long long)began);
        }
        if (awake_form == AWAKE_SPIN) {
            do {
                for (int i = 0; i < 1024; i++) {
                    awake_pause();
                }
                turns++;
            } while (!atomic_load(&awake_over) && awake_now());
        } else {
            uint64_t next = began;
            do {
                // A tick that came late is not made up for by ticks without a pause.
                uint64_t now = awake_clock();
                next = (next > now ? next : now) + AWAKE_TICK_NS;
                // The system's clock counts in its own units.
                mach_wait_until(next / base.numer * base.denom + next % base.numer * base.denom / base.numer);
                turns++;
            } while (!atomic_load(&awake_over) && awake_now());
        }
        uint64_t ended = awake_clock();
        awake_on_ns += ended - began;
        awake_stretches++;
        awake_turns += turns;
        if (awake_log != NULL) {
            fprintf(awake_log, "{\"kind\":\"awake\",\"t_ns\":%llu,\"on\":0,\"after_ms\":%.1f,\"turns\":%llu}\n",
                    (unsigned long long)ended, (ended - began) / 1e6, (unsigned long long)turns);
        }
    }
    awake_cpu_ns = clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID);
    return NULL;
}

// Starts the thread; it rests until the rule holds. False when it cannot be started.
static inline bool awake_start(void) {
    atomic_store(&awake_over, false);
    awake_on_ns = awake_stretches = awake_turns = awake_cpu_ns = 0;
    awake_started = pthread_create(&awake_runs, NULL, awake_thread, NULL) == 0;
    return awake_started;
}

// Ends the thread: it lets go of its core within a turn (spin: under a microsecond; tick: a
// millisecond). Then the log says what ran: how long the core was kept awake and in how many
// stretches, how often the thread went round (spin: 1024 pauses each; tick: one wake-up each),
// and the processor time the thread had.
static inline void awake_finish(void) {
    if (!awake_started) {
        return;
    }
    awake_started = false;
    atomic_store(&awake_over, true);
    pthread_mutex_lock(&awake_lock);
    pthread_cond_signal(&awake_changed);
    pthread_mutex_unlock(&awake_lock);
    pthread_join(awake_runs, NULL);
    if (awake_log != NULL) {
        fprintf(awake_log, "{\"kind\":\"wake_effect\",\"t_ns\":%llu,\"awake\":\"%s\",\"at\":\"end\",\"on_ms\":%.1f,\"stretches\":%llu,\"turns\":%llu,\"thread_cpu_ms\":%.1f}\n",
                (unsigned long long)awake_clock(), awake_word(), awake_on_ns / 1e6, (unsigned long long)awake_stretches,
                (unsigned long long)awake_turns, awake_cpu_ns / 1e6);
    }
}

#endif
