// What the process used (usage.h), without a window: the lines are written, a thread of this
// test is among them with the time it worked and the class it asked for, and the cores it was
// on are counted. One core is busy for a twentieth of a second.
#include <assert.h>
#include <stdbool.h>
#include <stdlib.h>
#include "usage.h"

static _Atomic bool worked = false, leave = false;
static const int looks = 1000;

static void *work(void *unused) {
    (void)unused;
    pthread_setname_np("usage test \"worker\"");
    assert(pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) == 0);
    uint64_t until = usage_clock() + 50000000ULL;
    for (int i = 0; i < looks || usage_clock() < until; i++) {
        if (i < looks) {
            usage_sample(i % 2);
        }
    }
    atomic_store(&worked, true);
    // Still there while the lines are written.
    while (!atomic_load(&leave)) {
        usleep(1000);
    }
    return NULL;
}

static double number_after(const char *line, const char *key) {
    const char *at = strstr(line, key);
    assert(at != NULL);
    return atof(at + strlen(key));
}

int main(void) {
    FILE *log = tmpfile();
    assert(log != NULL);
    usage_log_process(log, "start");
    pthread_t thread;
    assert(pthread_create(&thread, NULL, work, NULL) == 0);
    while (!atomic_load(&worked)) {
        usleep(1000);
    }
    usage_log_process(log, "end");
    usage_log_threads(log);
    usage_log_cores(log, "worker", "even", "odd");
    atomic_store(&leave, true);
    pthread_join(thread, NULL);

    char line[1024];
    double cpu_start = -1, cpu_end = -1, worker_cpu = -1, of_these = -1;
    int worker_qos = -1, asking = 0, threads = 0, cores_lines = 0;
    unsigned long long counted = 0;
    rewind(log);
    while (fgets(line, sizeof line, log) != NULL) {
        if (strstr(line, "\"usage\":\"start\"") != NULL) {
            assert(strstr(line, "\"read\":1") != NULL && strstr(line, "\"role\":") != NULL);
            cpu_start = number_after(line, "\"cpu_ms\":");
        } else if (strstr(line, "\"usage\":\"end\"") != NULL) {
            assert(strstr(line, "\"read\":1") != NULL && strstr(line, "\"energy_mj\":") != NULL);
            cpu_end = number_after(line, "\"cpu_ms\":");
        } else if (strstr(line, "\"kind\":\"usage_thread\"") != NULL && strstr(line, "\"name\":") != NULL) {
            threads++;
            asking += strstr(line, "\"this\":1") != NULL;
            // The name's quotation marks did not get into the log as they were.
            if (strstr(line, "\"name\":\"usage test ?worker?\"") != NULL) {
                worker_cpu = number_after(line, "\"cpu_ms\":");
                worker_qos = (int)number_after(line, "\"qos\":");
                assert(number_after(line, "\"priority\":") > 0);
            }
        } else if (strstr(line, "\"kind\":\"usage_thread\"") != NULL) {
            assert((int)number_after(line, "\"threads\":") == threads);
            of_these = number_after(line, "\"cpu_ms_of_these\":");
        } else if (strstr(line, "\"kind\":\"usage_cores\"") != NULL) {
            cores_lines++;
            assert(strstr(line, "\"thread\":\"worker\"") != NULL && strstr(line, "\"even\":[") != NULL && strstr(line, "\"odd\":[") != NULL);
            for (const char *at = strchr(line, '['); at != NULL; at = strchr(at + 1, '[')) {
                const char *digit = at + 1;
                while (*digit != ']' && *digit != '\0') {
                    char *end;
                    counted += strtoull(digit, &end, 10);
                    digit = *end == ',' ? end + 1 : end;
                }
            }
        }
    }
    // The process worked for the twentieth of a second the thread did, at least.
    assert(cpu_start >= 0 && cpu_end - cpu_start >= 40);
    // The thread is there by its name, with its time and the class it asked for (33:
    // user-interactive); so is the thread that asked, once.
    assert(worker_cpu >= 40 && worker_qos == 33 && asking == 1 && threads >= 2 && of_these >= worker_cpu);
    // Every look at the core was counted, half at each moment.
    assert(cores_lines == 1 && counted == (unsigned long long)looks);
    assert(usage_role() >= -1);
    printf("usage: the lines are written as described\n");
    return 0;
}
