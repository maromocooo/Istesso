// What the process used, as the system tells any program about itself (--wake usage;
// docs/adr/0018-own-client.md). For reading what a measuring switch costs and where the work
// runs, without root and with public interfaces only. Nothing here is called without the switch.
// Tested by usage-test.c, without a window. Included once, by one file.
//
// What the system does not tell through a public interface: how much of ONE thread's time was
// on the fast cores and how much on the efficient ones (it tells that for the process only),
// and a thread's effective scheduling class. In their place: each thread's processor time, the
// priority it is scheduled with, the class it asked for, and, for a thread that asks, the number
// of the core it was on at chosen moments.
#ifndef ISTESSO_USAGE_H
#define ISTESSO_USAGE_H

#include <libproc.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <pthread/qos.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
#include <time.h>
#include <unistd.h>

// The clock of the frame log.
static inline uint64_t usage_clock(void) {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

// How the system treats the process as a whole (mach/task_policy.h): 1 an application in the
// foreground, 2 one in the background, 0 not said, others as that header names them; -1 when
// it cannot be read.
static inline int usage_role(void) {
    struct task_category_policy policy = {TASK_UNSPECIFIED};
    mach_msg_type_number_t count = TASK_CATEGORY_POLICY_COUNT;
    boolean_t standard = FALSE;
    if (task_policy_get(mach_task_self(), TASK_CATEGORY_POLICY, (task_policy_t)&policy, &count, &standard) != KERN_SUCCESS) {
        return -1;
    }
    return (int)policy.role;
}

// The process: processor time, how much of it on the fast cores, and the energy the system
// bills to it. `at`: "start" or "end"; the difference is the session's. (The system's own
// figure for this process: it is not the power of the whole Mac.)
static inline void usage_log_process(FILE *log, const char *at) {
    struct rusage_info_v6 used;
    memset(&used, 0, sizeof used);
    if (proc_pid_rusage(getpid(), RUSAGE_INFO_V6, (rusage_info_t *)&used) != 0) {
        fprintf(log, "{\"kind\":\"wake_effect\",\"t_ns\":%llu,\"usage\":\"%s\",\"read\":0}\n", (unsigned long long)usage_clock(), at);
        return;
    }
    // The times are in the units of the system's clock, not in nanoseconds.
    mach_timebase_info_data_t base;
    mach_timebase_info(&base);
    double ms = (double)base.numer / base.denom / 1e6;
    fprintf(log, "{\"kind\":\"wake_effect\",\"t_ns\":%llu,\"usage\":\"%s\",\"read\":1,\"cpu_ms\":%.1f,\"fast_cpu_ms\":%.1f,\"energy_mj\":%.1f,\"fast_energy_mj\":%.1f,\"role\":%d}\n",
            (unsigned long long)usage_clock(), at, (used.ri_user_time + used.ri_system_time) * ms,
            (used.ri_user_ptime + used.ri_system_ptime) * ms, used.ri_energy_nj / 1e6, used.ri_penergy_nj / 1e6, usage_role());
}

// Every thread the process has at this moment, one line each: its name (the system's own
// threads have theirs), the processor time it has had, the priority it is scheduled with (its
// own, and the one in effect now), the policy (1 shares time, 2 and 4 are fixed: real time),
// and the class it asked for (33 user-interactive, 25 user-initiated, 21 default, 17 utility,
// 9 background, 0 none; -1 unknown). `this`: the thread that asks (the window's, at the end).
// Threads that have ended are not here; the last line has the sum of those that are.
static inline void usage_log_threads(FILE *log) {
    thread_act_array_t threads = NULL;
    mach_msg_type_number_t count = 0;
    if (task_threads(mach_task_self(), &threads, &count) != KERN_SUCCESS) {
        fprintf(log, "{\"kind\":\"usage_thread\",\"t_ns\":%llu,\"read\":0}\n", (unsigned long long)usage_clock());
        return;
    }
    thread_t asking = mach_thread_self();
    uint64_t sum_ns = 0;
    unsigned read = 0;
    for (mach_msg_type_number_t i = 0; i < count; i++) {
        struct thread_extended_info info;
        mach_msg_type_number_t size = THREAD_EXTENDED_INFO_COUNT;
        if (thread_info(threads[i], THREAD_EXTENDED_INFO, (thread_info_t)&info, &size) == KERN_SUCCESS) {
            qos_class_t asked = QOS_CLASS_UNSPECIFIED;
            pthread_t thread = pthread_from_mach_thread_np(threads[i]);
            int qos = thread != NULL && pthread_get_qos_class_np(thread, &asked, NULL) == 0 ? (int)asked : -1;
            // A name goes into the log as letters, digits and a few signs; anything else as "?".
            char name[MAXTHREADNAMESIZE + 1];
            size_t length = strnlen(info.pth_name, MAXTHREADNAMESIZE);
            for (size_t c = 0; c < length; c++) {
                char sign = info.pth_name[c];
                int plain = (sign >= 'a' && sign <= 'z') || (sign >= 'A' && sign <= 'Z') || (sign >= '0' && sign <= '9')
                            || sign == ' ' || sign == '.' || sign == '-' || sign == '_' || sign == ':';
                name[c] = plain ? sign : '?';
            }
            name[length] = '\0';
            uint64_t cpu_ns = info.pth_user_time + info.pth_system_time;
            sum_ns += cpu_ns;
            read++;
            fprintf(log, "{\"kind\":\"usage_thread\",\"t_ns\":%llu,\"name\":\"%s\",\"this\":%d,\"cpu_ms\":%.1f,\"priority\":%d,\"priority_now\":%d,\"policy\":%d,\"qos\":%d}\n",
                    (unsigned long long)usage_clock(), name, threads[i] == asking, cpu_ns / 1e6, info.pth_priority, info.pth_curpri,
                    info.pth_policy, qos);
        }
        mach_port_deallocate(mach_task_self(), threads[i]);
    }
    mach_port_deallocate(mach_task_self(), asking);
    vm_deallocate(mach_task_self(), (vm_address_t)threads, count * sizeof(thread_t));
    fprintf(log, "{\"kind\":\"usage_thread\",\"t_ns\":%llu,\"read\":1,\"threads\":%u,\"cpu_ms_of_these\":%.1f}\n",
            (unsigned long long)usage_clock(), read, sum_ns / 1e6);
}

// ---- Which core a thread is on at chosen moments ----
//
// The thread asks for itself (one instruction's worth), and the counts are written at the end:
// how often it was on core 0, core 1, ... at each of the moments. Which numbers are the fast
// cores is the Mac's: `ioreg -p IODeviceTree -n cpu0 -r -d 1` (and cpu1, ...) says "cluster-type"
// E or P for each number.
#define USAGE_MOMENTS 2
#define USAGE_CORES 64
static _Atomic uint64_t usage_cores[USAGE_MOMENTS][USAGE_CORES];

static inline void usage_sample(int moment) {
    size_t core = 0;
    if (pthread_cpu_number_np(&core) == 0 && core < USAGE_CORES) {
        atomic_fetch_add_explicit(&usage_cores[moment][core], 1, memory_order_relaxed);
    }
}

// `thread`, `first`, `second`: the words for the thread and the two moments.
static inline void usage_log_cores(FILE *log, const char *thread, const char *first, const char *second) {
    long cores = sysconf(_SC_NPROCESSORS_CONF);
    if (cores < 1 || cores > USAGE_CORES) {
        cores = USAGE_CORES;
    }
    const char *words[USAGE_MOMENTS] = {first, second};
    fprintf(log, "{\"kind\":\"usage_cores\",\"t_ns\":%llu,\"thread\":\"%s\"", (unsigned long long)usage_clock(), thread);
    for (int moment = 0; moment < USAGE_MOMENTS; moment++) {
        fprintf(log, ",\"%s\":[", words[moment]);
        for (long core = 0; core < cores; core++) {
            fprintf(log, "%s%llu", core > 0 ? "," : "",
                    (unsigned long long)atomic_load_explicit(&usage_cores[moment][core], memory_order_relaxed));
        }
        fprintf(log, "]");
    }
    fprintf(log, "}\n");
}

#endif
